use super::{Identity, Resolver};
use crate::analysis::member::MemberDeclaration;
use crate::analysis::method_set::ReceiverMember;
use crate::analysis::scope::{self as locals, Locals};
use crate::analysis::types::TypeForm;
use crate::syntax::{MemberKind, TokenKind};
use crate::text::SourceMap;
use std::ops::Range;

pub(super) enum LiteralKey<'a> {
    Field(MemberDeclaration<'a>),
    Expression,
    Unknown,
}

impl<'a> Resolver<'a> {
    pub(super) fn member(&mut self, receiver: &str, name: &str) -> Option<Identity<'a>> {
        let members = self.members.entry(receiver.to_owned()).or_insert_with(|| {
            if self.index.is_some() {
                self.workspace
                    .receiver_member(self.file, receiver, name)
                    .into_iter()
                    .collect()
            } else {
                self.workspace.receiver_members(self.file, receiver)
            }
        });
        members.iter().find_map(|member| match member {
            ReceiverMember::Method(path, symbol) if symbol.name == name => {
                Some(Identity::Global(path, symbol))
            }
            ReceiverMember::Declaration(member) if member.name == name => {
                Some(Identity::Member(*member))
            }
            _ => None,
        })
    }

    pub(super) fn field_declaration(
        &self,
        start: usize,
        name: &str,
    ) -> Option<MemberDeclaration<'a>> {
        if !self.members_enabled || self.package_global_only {
            return None;
        }
        let (file, stub) = self.workspace.files.get_key_value(self.file)?;
        let owner = stub
            .symbols
            .iter()
            .find(|symbol| symbol.start == start && symbol.is_type())?;
        let info = owner.type_info.as_ref()?;
        let mut names = info.members.iter().filter(|member| *member == name);
        let name = names.next()?;
        if names.next().is_some() {
            return None;
        }
        Some(MemberDeclaration {
            file,
            owner,
            name,
            kind: if info.is_interface() {
                MemberKind::Method
            } else {
                MemberKind::Field
            },
            embedded: false,
        })
    }

    pub(super) fn literal_key(
        &mut self,
        source: &str,
        locals: &Locals,
        ty: &Range<usize>,
        name: &str,
    ) -> LiteralKey<'a> {
        self.literal_key_type(source, locals, &source[ty.clone()], ty.start, name)
    }

    pub(super) fn literal_key_type(
        &mut self,
        source: &str,
        locals: &Locals,
        ty: &str,
        offset: usize,
        name: &str,
    ) -> LiteralKey<'a> {
        let Some(ty_name) = locals::named_type(ty) else {
            return if self
                .workspace
                .type_form(self.file, ty, locals, offset)
                .is_some_and(|form| {
                    matches!(form, TypeForm::Map | TypeForm::Array | TypeForm::Slice)
                }) {
                LiteralKey::Expression
            } else {
                LiteralKey::Unknown
            };
        };
        if let Some(binding) = locals.resolve_at(
            offset,
            locals::named_root(&ty_name)
                .split('.')
                .next()
                .expect("named type"),
        ) {
            return if binding.object_kind.is_type()
                && binding
                    .annotation()
                    .and_then(|span| source.get(span.clone()))
                    .is_some_and(map_or_array)
            {
                LiteralKey::Expression
            } else {
                LiteralKey::Unknown
            };
        }
        let Some((file, owner)) = self.workspace.composite_target(self.file, &ty_name) else {
            return LiteralKey::Unknown;
        };
        if owner.type_info.is_some() {
            if self.package_global_only || !self.members_enabled {
                return LiteralKey::Unknown;
            }
            return match self.member(&ty_name, name) {
                Some(Identity::Member(member))
                    if member.file == file
                        && std::ptr::eq(member.owner, owner)
                        && member.kind == MemberKind::Field =>
                {
                    LiteralKey::Field(member)
                }
                _ => LiteralKey::Unknown,
            };
        }
        let Some(source) = self.workspace.source(file) else {
            return LiteralKey::Unknown;
        };
        let map = SourceMap::new(&source);
        let proven = map
            .offset(owner.range.end)
            .and_then(|end| source.get(owner.end..end))
            .is_some_and(map_or_array);
        if proven {
            LiteralKey::Expression
        } else {
            LiteralKey::Unknown
        }
    }
}

fn map_or_array(source: &str) -> bool {
    crate::syntax::Lexer::new(source)
        .next()
        .is_some_and(|token| matches!(token.kind, TokenKind::Map | TokenKind::LBrack))
}
