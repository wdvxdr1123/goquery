//! Checked completion labels/types share query scope, never protocol item edits.
use super::signatures::signature_in_source;
use super::{
    CallableSignature, CompletionReceiver, Context, Kind, PreparedMethods, Presentation,
    SignatureSource,
};
use crate::analysis::obj::{MemberDeclaration, ReceiverMember};
use crate::analysis::scope::{Binding, Locals};
use crate::analysis::types::lookup::ScopedOwnerEnv;
use crate::analysis::types::{
    TypeForm, TypePresentation, completion_inferable, declared_type_presentation,
    local_type_presentation,
};
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{AggregateMember, MemberKind, TokenKind};
use crate::text::SourceMap;
use std::path::Path;

impl Context<'_> {
    pub(super) fn generic_completion(
        &self,
        location: &SignatureSource<'_>,
        symbol: &Symbol,
    ) -> Option<Presentation> {
        let start = location.map.offset(symbol.range.start)?;
        let end = symbol
            .body()
            .map(|body| body.start)
            .or_else(|| location.map.offset(symbol.range.end))?;
        let header = location
            .source
            .get(start..end)
            .filter(|text| text.len() <= 16 * 1024)?;
        let signature = crate::syntax::parse_signature(header);
        if !signature.receiver.is_empty() || !signature.diagnostics.is_empty() {
            return None;
        }
        let formals: Vec<_> = signature
            .type_parameters
            .iter()
            .map(|parameter| {
                Some((
                    header.get(parameter.name.as_ref()?.clone())?,
                    header.get(parameter.annotation.clone())?,
                ))
            })
            .collect::<Option<_>>()?;
        let inputs: Vec<_> = signature
            .parameters
            .iter()
            .map(|parameter| &header[parameter.annotation.clone()])
            .collect();
        // Inference uncertainty must keep explicit arguments, never erase them.
        let inferred = completion_inferable(self.workspace, location.origin, &formals, &inputs)
            .unwrap_or_else(|| vec![false; formals.len()]);
        let keep = inferred
            .iter()
            .rposition(|inferred| !inferred)
            .map_or(0, |index| index + 1);
        let names: Vec<_> = formals.iter().map(|(name, _)| *name).collect();
        let type_parameters = formals[..keep]
            .iter()
            .map(|(name, constraint)| {
                Some(format!(
                    "{name} {}",
                    crate::analysis::query::signatures::label::signature_type_scoped(
                        self.workspace,
                        location.file,
                        location.origin,
                        constraint,
                        &names
                    )?
                ))
            })
            .collect::<Option<_>>()?;
        Some(Presentation {
            detail: None,
            signature: Some(
                crate::analysis::query::signatures::label::declared_signature(
                    self.workspace,
                    location.file,
                    location.origin,
                    header,
                    &signature,
                    &symbol.name,
                )?,
            ),
            type_parameters: Some(type_parameters),
            kind: None,
        })
    }
}

impl Context<'_> {
    pub(super) fn aggregate_type_completion(
        &self,
        file: &Path,
        symbol: &Symbol,
    ) -> Option<Presentation> {
        let info = symbol
            .type_info
            .as_ref()
            .filter(|info| info.target.is_none())?;
        self.type_presentation(
            file,
            TypePresentation {
                form: if info.is_interface() {
                    TypeForm::Interface
                } else {
                    TypeForm::Struct
                },
                text: if info.is_interface() {
                    "interface{...}"
                } else {
                    "struct{...}"
                }
                .to_owned(),
            },
        )
    }
    pub(super) fn type_completion(
        &self,
        file: &Path,
        origin: &Path,
        symbol: &Symbol,
        source: &str,
        map: &SourceMap<'_>,
    ) -> Option<Presentation> {
        let declaration = source.get(symbol.end..map.offset(symbol.range.end)?)?;
        let presentation = declared_type_presentation(self.workspace, file, origin, declaration)?;
        self.type_presentation(file, presentation)
    }

    pub(super) fn type_presentation(
        &self,
        file: &Path,
        presentation: TypePresentation,
    ) -> Option<Presentation> {
        let kind = match presentation.form {
            TypeForm::Struct => Kind::Struct,
            TypeForm::Interface => Kind::Interface,
            _ => Kind::TypeName,
        };
        let detail = if matches!(presentation.form, TypeForm::Signature) {
            let parsed = crate::syntax::parse_literal_signature(&presentation.text);
            let (information, _) =
                crate::analysis::query::signatures::label::signature_from_parsed(
                    self.workspace,
                    file,
                    file,
                    &presentation.text,
                    &parsed,
                    "",
                )?;
            format!("func{}", information.label)
        } else {
            presentation.text
        };
        Some(Presentation {
            detail: Some(detail),
            signature: None,
            type_parameters: None,
            kind: Some(kind),
        })
    }
}

impl Context<'_> {
    pub(crate) fn member_presentation(
        &self,
        types: Option<&ScopedOwnerEnv<'_>>,
        member: &MemberDeclaration<'_>,
        declaration: &str,
        entries: &[AggregateMember],
    ) -> Option<Presentation> {
        let file = self.file();
        let entry = member.entry(declaration, entries)?;
        let annotation = declaration.get(entry.annotation.clone())?.trim();
        if annotation.is_empty() || annotation.len() > 16 * 1024 {
            return None;
        }
        let callable = member.kind == MemberKind::Method
            || crate::syntax::Lexer::new(annotation)
                .next()
                .is_some_and(|token| token.kind == TokenKind::Func);
        let header = if member.kind == MemberKind::Method {
            format!("func {annotation}")
        } else {
            annotation.to_owned()
        };
        let generic = crate::syntax::Lexer::new(declaration)
            .next()
            .is_some_and(|token| token.kind == TokenKind::LBrack);
        let instantiated = if generic {
            Some(types?.render(&header)?)
        } else {
            None
        };
        let origin = if instantiated.is_some() {
            file
        } else {
            member.file
        };
        let header = instantiated.as_deref().unwrap_or(&header);
        if callable {
            let parsed = crate::syntax::parse_literal_signature(header);
            let signature = crate::analysis::query::signatures::label::signature_from_parsed(
                self.workspace,
                file,
                origin,
                header,
                &parsed,
                member.name,
            )?;
            let opening = signature.0.label.find('(')?;
            let detail = Some(format!("func{}", &signature.0.label[opening..]));
            return Some(Presentation {
                detail,
                signature: (member.kind == MemberKind::Method).then_some(signature),
                type_parameters: None,
                kind: None,
            });
        }
        Some(Presentation {
            detail: crate::analysis::query::signatures::label::signature_type(
                self.workspace,
                file,
                origin,
                header,
            ),
            signature: None,
            type_parameters: None,
            kind: None,
        })
    }
}

impl<'a> Context<'a> {
    pub(crate) fn aggregate_members(&self, declaration: &str) -> Vec<AggregateMember> {
        crate::syntax::aggregate_members(declaration)
    }
    pub(crate) fn completion_members(
        &self,
        receiver: &CompletionReceiver<'_>,
    ) -> Vec<ReceiverMember<'a>> {
        crate::analysis::types::lookup::scoped_receiver_members(
            self.workspace,
            self.file(),
            receiver.scope,
            &receiver.text,
        )
    }
    pub(crate) fn completion_member_types(
        &self,
        scope: &Path,
        receiver: &str,
        member: MemberDeclaration<'_>,
        declaration: &str,
    ) -> Option<ScopedOwnerEnv<'a>> {
        crate::syntax::Lexer::new(declaration)
            .next()
            .filter(|token| token.kind == TokenKind::LBrack)?;
        ScopedOwnerEnv::for_member(self.workspace, self.file(), scope, receiver, member)
    }
    pub(crate) fn binding_signature(
        &self,
        _locals: &Locals,
        binding: &Binding,
    ) -> Option<(CallableSignature, bool)> {
        self.local_binding_signature(&binding.selection, binding.scope_start(), &binding.name)
    }
    pub(crate) fn binding_detail(&self, locals: &Locals, binding: &Binding) -> Option<String> {
        self.binding_type(locals, binding)
    }
    pub(crate) fn local_completion_type(
        &self,
        locals: &Locals,
        binding: &Binding,
    ) -> Option<Presentation> {
        let presentation =
            local_type_presentation(self.workspace, self.file(), self.source(), locals, binding)?;
        self.type_presentation(self.file(), presentation)
    }
    pub(crate) fn symbol_completion(
        &self,
        symbol: &Symbol,
        location: Option<&SignatureSource<'_>>,
        receiver: Option<(&Path, &str)>,
        methods: &mut Option<PreparedMethods<'a>>,
    ) -> Option<Presentation> {
        if symbol.is_callable() {
            let location = location?;
            if crate::syntax::Lexer::new(&location.source[symbol.end..])
                .next()
                .is_some_and(|token| token.kind == TokenKind::LBrack)
            {
                return self.generic_completion(location, symbol);
            }
            signature_in_source(self.workspace, location, symbol, receiver, methods).map(
                |signature| Presentation {
                    detail: None,
                    signature: Some(signature),
                    type_parameters: None,
                    kind: None,
                },
            )
        } else if symbol.is_type() {
            self.aggregate_type_completion(self.file(), symbol)
                .or_else(|| {
                    let location = location?;
                    self.type_completion(
                        location.file,
                        location.origin,
                        symbol,
                        location.source,
                        location.map,
                    )
                })
        } else {
            None
        }
    }
}
