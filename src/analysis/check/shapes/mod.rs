//! Bounded outer-type queries; only small modifier results survive within a request.
pub(crate) mod kinds;
mod operations;
mod value;
use self::kinds as config;
use crate::analysis::check::Inference;
use crate::analysis::member::MemberDeclaration;
use crate::analysis::scope::{self as locals, Binding, DeclarationKind, Identity};
use crate::analysis::source::{Name, Role, SourceAnalysis};
use crate::analysis::types::{TypeArena, TypeForm};
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{Lexer, TokenKind};
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Typedness {
    Typed,
    Untyped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ValueShape {
    pub(crate) forms: u64,
    typedness: Typedness,
}

impl ValueShape {
    const fn typed(forms: u64) -> Self {
        Self {
            forms,
            typedness: Typedness::Typed,
        }
    }

    const fn untyped(forms: u64) -> Self {
        Self {
            forms,
            typedness: Typedness::Untyped,
        }
    }

    fn into_typed(mut self) -> Self {
        self.typedness = Typedness::Typed;
        self
    }
}

impl Default for ValueShape {
    fn default() -> Self {
        Self::typed(0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key<'a> {
    Local(usize, usize),
    Global(&'a Path, usize),
    Member(&'a Path, usize, &'a str),
}

pub(crate) struct Shapes<'a> {
    workspace: &'a Workspace,
    analysis: &'a SourceAnalysis,
    file: &'a Path,
    source: &'a str,
    cache: HashMap<Key<'a>, ValueShape>,
    annotations: HashMap<&'a Path, HashMap<String, ValueShape>>,
    annotation_count: usize,
    annotation_bytes: usize,
}

impl<'a> Shapes<'a> {
    pub fn new(
        workspace: &'a Workspace,
        analysis: &'a SourceAnalysis,
        file: &'a Path,
        source: &'a str,
    ) -> Self {
        Self {
            workspace,
            analysis,
            file,
            source,
            cache: HashMap::new(),
            annotations: HashMap::new(),
            annotation_count: 0,
            annotation_bytes: 0,
        }
    }

    pub fn style(&mut self, name: &Name, identity: &Option<Identity<'a>>) -> ValueShape {
        let constant = match identity {
            Some(Identity::Local(selection)) => self
                .analysis
                .locals
                .definition_at(selection, &self.source[name.span.clone()])
                .filter(|binding| binding.selection == *selection)
                .is_some_and(|binding| binding.declaration == DeclarationKind::Constant),
            Some(Identity::Global(_, symbol)) => symbol.is_const(),
            None => matches!(
                &self.source[name.span.clone()],
                "true" | "false" | "iota" | "nil"
            ),
            _ => false,
        };
        let value = match identity {
            Some(Identity::Local(selection)) => {
                if let Some(binding) = self
                    .analysis
                    .locals
                    .resolve_at(name.span.start, &self.source[name.span.clone()])
                    .filter(|binding| binding.selection == *selection)
                    .or_else(|| {
                        self.analysis
                            .locals
                            .definition_at(selection, &self.source[name.span.clone()])
                    })
                    .filter(|binding| binding.selection == *selection)
                {
                    if binding.selection == name.span
                        && self.analysis.locals.switch_case(binding).is_some()
                    {
                        return ValueShape::default();
                    }
                    self.binding(binding, 0)
                } else if let Some((span, variadic)) = self.analysis.signature_type(selection.start)
                {
                    if variadic {
                        ValueShape::typed(config::SLICE)
                    } else {
                        self.annotation(self.file, &self.source[span.clone()], span.start, 0)
                    }
                } else {
                    ValueShape::default()
                }
            }
            Some(Identity::Global(file, symbol)) => self.symbol(file, symbol, 0),
            Some(Identity::Member(member)) => self.member(name, *member),
            None if matches!(name.role, Role::Name | Role::Type(None)) => {
                let text = &self.source[name.span.clone()];
                if text == "_"
                    && let Some(binding) = self.analysis.locals.range_blank_at(&name.span)
                {
                    return self.binding(binding, 0).into_typed();
                }
                if crate::analysis::scope::universe::imported_name(self.workspace, self.file, text)
                {
                    return ValueShape::default();
                }
                match text {
                    "true" | "false" => ValueShape::untyped(config::BOOL),
                    "iota" => ValueShape::untyped(config::NUMBER),
                    _ => self.annotation(self.file, text, name.span.start, 0),
                }
            }
            _ => ValueShape::default(),
        };
        // Untyped bool/string constants do not carry gopls's typed basic flags.
        // Retain the form in cached values so variable defaulting can recover it.
        if constant && value.typedness == Typedness::Untyped {
            ValueShape::typed(value.forms & !(config::BOOL | config::STRING))
        } else {
            value.into_typed()
        }
    }

    fn store(&mut self, key: Key<'a>, value: ValueShape) {
        if self.cache.len() + self.annotation_count < 512 || self.cache.contains_key(&key) {
            self.cache.insert(key, value);
        }
    }

    fn member(&mut self, name: &Name, member: MemberDeclaration<'a>) -> ValueShape {
        if member.kind == crate::syntax::MemberKind::Method {
            return ValueShape::typed(config::SIGNATURE);
        }
        let key = Key::Member(member.file, member.owner.start, member.name);
        if let Some(value) = self.cache.get(&key) {
            return *value;
        }
        let (annotation, generic) = {
            let stored = (member.file != self.file)
                .then(|| self.workspace.source(member.file))
                .flatten();
            let source = if member.file == self.file {
                self.source
            } else {
                match stored.as_deref() {
                    Some(source) => source,
                    None => return ValueShape::default(),
                }
            };
            let generic = Lexer::new(&source[member.owner.end..])
                .next()
                .is_some_and(|token| token.kind == TokenKind::LBrack);
            let map = SourceMap::new(source);
            let Some(data) = Workspace::member_data_in_source(member, source, &map) else {
                return ValueShape::default();
            };
            let Some(annotation) = data.annotation else {
                return ValueShape::default();
            };
            (annotation, generic)
        };
        let value = if generic && !matches!(name.role, Role::FieldDeclaration(_)) {
            let receiver = match &name.role {
                Role::Member(Some(span)) | Role::ExpressionMember(span) => {
                    let Some(text) = self
                        .source
                        .get(span.start..name.span.end)
                        .filter(|text| text.len() <= 16 * 1024)
                    else {
                        return ValueShape::default();
                    };
                    let parsed = crate::syntax::parse_expression(text);
                    if !parsed.diagnostics.is_empty() {
                        return ValueShape::default();
                    }
                    Inference::new(
                        self.workspace,
                        self.file,
                        self.source,
                        &self.analysis.locals,
                        &parsed.nodes,
                        span.start,
                    )
                    .receiver_type(parsed.root)
                }
                Role::LiteralKey(span) => locals::named_type(&self.source[span.clone()]),
                Role::LiteralElementKey(id) => self
                    .analysis
                    .literal_element_type(self.workspace, self.file, self.source, *id)
                    .and_then(|(ty, _)| locals::named_type(&ty)),
                _ => None,
            };
            let Some(receiver) = receiver else {
                return ValueShape::default();
            };
            let Some(mut types) = self.workspace.member_types(self.file, &receiver, member) else {
                return ValueShape::default();
            };
            types
                .form(&annotation, &self.analysis.locals, name.span.start)
                .map_or(ValueShape::default(), modifier)
        } else {
            self.annotation(member.file, &annotation, name.span.start, 0)
        };
        // A generic owner's field can have different forms for each receiver.
        if !generic {
            self.store(key, value);
        }
        value
    }

    fn binding(&mut self, binding: &Binding, depth: usize) -> ValueShape {
        if binding.declaration == DeclarationKind::Variadic {
            return ValueShape::typed(config::SLICE);
        }
        if depth >= 32 || binding.declaration == DeclarationKind::TypeParameter {
            return ValueShape::default();
        }
        let key = Key::Local(binding.selection.start, binding.scope_start());
        if let Some(value) = self.cache.get(&key) {
            return *value;
        }
        self.store(key, ValueShape::default());
        let value = if self.analysis.locals.switch_case(binding).is_some() {
            let inference = Inference::new(
                self.workspace,
                self.file,
                self.source,
                &self.analysis.locals,
                &[],
                0,
            );
            let mut arena = TypeArena::new(self.workspace);
            inference
                .switch_operand(&mut arena, binding)
                .and_then(|value| arena.form(value.ty))
                .map_or(ValueShape::default(), modifier)
        } else if let Some(span) = binding.annotation() {
            self.annotation(self.file, &self.source[span.clone()], span.start, depth + 1)
        } else if binding.range_initializer().is_some() {
            self.workspace
                .binding_type(self.file, self.source, &self.analysis.locals, binding)
                .map_or(ValueShape::default(), |text| {
                    self.annotation(self.file, &text, binding.selection.start, depth + 1)
                })
        } else if let Some((span, result, count)) = binding.initializer() {
            self.value_slot(
                self.file,
                &self.source[span.clone()],
                span.start,
                result,
                count,
                depth + 1,
            )
        } else {
            ValueShape::default()
        };
        let value = if binding.declaration == DeclarationKind::Value {
            value.into_typed()
        } else {
            value
        };
        self.store(key, value);
        value
    }

    fn annotation(&mut self, file: &Path, text: &str, offset: usize, depth: usize) -> ValueShape {
        if depth >= 32 || text.len() > 16 * 1024 {
            return ValueShape::default();
        }
        if file == self.file
            && Lexer::new(text)
                .next()
                .is_some_and(|token| token.kind == TokenKind::Ident)
            && let Some(name) = locals::named_type(text)
            && let Some(binding) = self.analysis.locals.resolve_at(
                offset,
                locals::named_root(&name).split('.').next().unwrap_or(""),
            )
        {
            return if binding.declaration == DeclarationKind::Type
                && name == locals::named_root(&name)
            {
                self.binding(binding, depth + 1)
            } else {
                ValueShape::default()
            };
        }
        let empty = locals::Locals::empty();
        let scope = if file == self.file {
            &self.analysis.locals
        } else {
            &empty
        };
        // Local roots were handled above. A non-instantiated named spelling
        // now has a stable meaning in this file's package/import scope.
        let named = Lexer::new(text)
            .next()
            .is_some_and(|token| token.kind == TokenKind::Ident)
            && !text.contains('[');
        if named && let Some(value) = self.annotations.get(file).and_then(|cache| cache.get(text)) {
            return *value;
        }
        let value = self
            .workspace
            .type_form(file, text, scope, offset)
            .map_or(ValueShape::default(), modifier);
        if named
            && self.cache.len() + self.annotation_count < 512
            && self.annotation_bytes + text.len() <= 256 * 1024
            && let Some((file, _)) = self.workspace.files.get_key_value(file)
        {
            self.annotations
                .entry(file)
                .or_default()
                .insert(text.to_owned(), value);
            self.annotation_count += 1;
            self.annotation_bytes += text.len();
        }
        value
    }

    fn symbol(&mut self, file: &'a Path, symbol: &'a Symbol, depth: usize) -> ValueShape {
        if depth >= 32 {
            return ValueShape::default();
        }
        if symbol.is_callable() {
            return ValueShape::typed(config::SIGNATURE);
        }
        let key = Key::Global(file, symbol.start);
        if let Some(value) = self.cache.get(&key) {
            return *value;
        }
        self.store(key, ValueShape::default());
        let value = if symbol.is_type() {
            self.workspace
                .type_form(file, &symbol.name, &locals::Locals::empty(), 0)
                .map_or(ValueShape::default(), modifier)
        } else {
            self.global_value(file, symbol, depth + 1)
        };
        let value = if symbol.is_var() {
            value.into_typed()
        } else {
            value
        };
        self.store(key, value);
        value
    }
}

fn modifier(form: TypeForm) -> ValueShape {
    ValueShape::typed(match form {
        TypeForm::Number => config::NUMBER,
        TypeForm::Bool => config::BOOL,
        TypeForm::String => config::STRING,
        TypeForm::Struct => config::STRUCT,
        TypeForm::Interface => config::INTERFACE,
        TypeForm::Signature => config::SIGNATURE,
        TypeForm::Pointer => config::POINTER,
        TypeForm::Array => config::ARRAY,
        TypeForm::Slice => config::SLICE,
        TypeForm::Map => config::MAP,
        TypeForm::Chan => config::CHAN,
    })
}
