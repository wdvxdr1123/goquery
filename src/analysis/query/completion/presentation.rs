//! Checked completion labels/types share query scope, never protocol item edits.
use super::signatures::signature_in_source;
use super::{CallableSignature, Context, Kind, Presentation, SignatureSource};
use crate::analysis::scope::{Binding, Locals};
use crate::analysis::types::{TypeForm, completion_inferable};
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::{AggregateMember, TokenKind};
use crate::text::SourceMap;

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
        let inferred = self
            .with_sema(|sema| {
                completion_inferable(sema.arena_mut(), location.origin, &formals, &inputs)
            })
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
    fn type_presentation(
        &self,
        view: crate::analysis::sema::CompletionTypeView,
    ) -> Option<Presentation> {
        let kind = match view.form {
            TypeForm::Struct => Kind::Struct,
            TypeForm::Interface => Kind::Interface,
            _ => Kind::TypeName,
        };
        let detail = if let Some(callable) = view.callable {
            let signature = view
                .header
                .as_deref()
                .and_then(|header| {
                    crate::analysis::query::signatures::label::signature_from_view(
                        header,
                        &crate::syntax::parse_literal_signature(header),
                        "",
                        &callable,
                    )
                })
                .or_else(|| {
                    crate::analysis::query::signatures::label::unnamed_signature_view("", &callable)
                })?;
            format!("func{}", signature.0.label)
        } else {
            view.text
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
    pub(in crate::analysis::query) fn aggregate_members(
        &self,
        declaration: &str,
    ) -> Vec<AggregateMember> {
        crate::syntax::aggregate_members(declaration)
    }

    pub(super) fn member_presentation(
        &self,
        member: &super::CompletionMember<'_>,
        view: Option<&crate::analysis::sema::MemberTypeView>,
        source: Option<&str>,
        map: Option<&SourceMap<'_>>,
        declaration: &str,
        entries: &[AggregateMember],
    ) -> Option<Presentation> {
        let view = view?;
        let is_method = matches!(member.object, crate::analysis::obj::ObjectRef::Method(_));
        let signature = view.callable.as_ref().and_then(|callable| {
            let header = member.method.and_then(|(_, method)| {
                let source = source?;
                let start = map?.offset(method.range.start)?;
                let end = method
                    .body()
                    .map(|body| body.start)
                    .or_else(|| map?.offset(method.range.end))?;
                let header = source.get(start..end)?;
                (header.len() <= 16 * 1024).then_some(header)
            });
            let annotation = member.declaration.and_then(|member| {
                let entry = member.entry(declaration, entries)?;
                declaration.get(entry.annotation.clone())
            });
            if !is_method
                && annotation.is_none_or(|annotation| {
                    crate::syntax::Lexer::new(annotation)
                        .next()
                        .is_none_or(|token| token.kind != TokenKind::Func)
                })
            {
                return None;
            }
            let owned_header = annotation
                .filter(|_| is_method)
                .map(|annotation| format!("func {annotation}"));
            let header = header.or(owned_header.as_deref()).or_else(|| {
                annotation.filter(|annotation| {
                    crate::syntax::Lexer::new(annotation)
                        .next()
                        .is_some_and(|token| token.kind == TokenKind::Func)
                })
            });
            let named = header.and_then(|header| {
                let mut parsed = if member.method.is_some() {
                    crate::syntax::parse_signature(header)
                } else {
                    crate::syntax::parse_literal_signature(header)
                };
                if view.method_expression {
                    parsed.parameters.insert(
                        0,
                        crate::syntax::Parameter {
                            name: parsed
                                .receiver
                                .first()
                                .and_then(|receiver| receiver.name.clone()),
                            annotation: 0..0,
                            variadic: false,
                        },
                    );
                }
                crate::analysis::query::signatures::label::signature_from_view(
                    header,
                    &parsed,
                    &member.name,
                    callable,
                )
            });
            named.or_else(|| {
                crate::analysis::query::signatures::label::unnamed_signature_view(
                    &member.name,
                    callable,
                )
            })
        });
        let detail = signature
            .as_ref()
            .and_then(|(signature, _)| {
                let opening = signature.label.find('(')?;
                Some(format!("func{}", &signature.label[opening..]))
            })
            .unwrap_or_else(|| view.type_text.clone());
        Some(Presentation {
            detail: Some(detail),
            signature: signature.filter(|_| is_method),
            type_parameters: None,
            kind: None,
        })
    }
}

impl Context<'_> {
    pub(in crate::analysis::query) fn binding_signature(
        &self,
        _locals: &Locals,
        binding: &Binding,
    ) -> Option<(CallableSignature, bool)> {
        self.local_binding_signature(&binding.selection, binding.scope_start(), &binding.name)
    }
    pub(in crate::analysis::query) fn binding_detail(
        &self,
        locals: &Locals,
        binding: &Binding,
    ) -> Option<String> {
        self.binding_type(locals, binding)
    }
    pub(in crate::analysis::query) fn local_completion_type(
        &self,
        _locals: &Locals,
        binding: &Binding,
    ) -> Option<Presentation> {
        let span = [
            u32::try_from(binding.selection.start).ok()?,
            u32::try_from(binding.selection.end).ok()?,
        ];
        let view =
            self.with_sema(|sema| sema.local_completion_type(self.file(), self.source(), span))?;
        self.type_presentation(view)
    }
    pub(in crate::analysis::query) fn symbol_completion(
        &self,
        origin: &std::path::Path,
        symbol: &Symbol,
        location: Option<&SignatureSource<'_>>,
    ) -> Option<Presentation> {
        if symbol.is_callable() {
            let location = location?;
            if crate::syntax::Lexer::new(&location.source[symbol.end..])
                .next()
                .is_some_and(|token| token.kind == TokenKind::LBrack)
            {
                return self.generic_completion(location, symbol);
            }
            signature_in_source(self.workspace, location, symbol).map(|signature| Presentation {
                detail: None,
                signature: Some(signature),
                type_parameters: None,
                kind: None,
            })
        } else if symbol.is_type() {
            let view = self.with_sema(|sema| {
                sema.declaration_completion_type(
                    crate::analysis::obj::DeclId {
                        file: origin,
                        symbol,
                    },
                    self.file(),
                )
            })?;
            self.type_presentation(view)
        } else {
            None
        }
    }
}
