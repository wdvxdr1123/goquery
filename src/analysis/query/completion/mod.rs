//! Completion semantic facts remain independent of snippets and wire items.
mod candidates;
mod comments;
mod context;
mod items;
mod presentation;
mod signatures;
mod type_position;
use super::Context;
use crate::analysis::resolve::MemberDescription;
use crate::analysis::sema::{MethodOrigin, SemanticObject};
use crate::index::IndexedSymbol;

/// Source metadata decorates a proven selection; its type stays native.
pub(in crate::analysis::query) struct CompletionMember<'a> {
    pub(crate) name: String,
    pub(crate) object: SemanticObject<'a>,
    pub(crate) method: Option<(&'a std::path::Path, &'a IndexedSymbol)>,
    pub(crate) declaration: Option<MemberDescription<'a>>,
    pub(crate) field_key: Option<FieldKey>,
    pub(crate) field_order: Option<u16>,
}

impl CompletionMember<'_> {
    fn origin(&self) -> &std::path::Path {
        match self.object {
            SemanticObject::Method(MethodOrigin::Concrete(decl)) => decl.file,
            SemanticObject::Field(member)
            | SemanticObject::Method(MethodOrigin::Interface(member)) => member.file,
            _ => unreachable!("only native members enter completion"),
        }
    }
}
pub(in crate::analysis::query) struct CompletionMembers<'a> {
    pub(crate) site: crate::analysis::sema::CompletionSite,
    pub(crate) used: Vec<[u32; 2]>,
    pub(crate) offset: u32,
    pub(crate) members: Vec<CompletionMember<'a>>,
}
impl CompletionMembers<'_> {
    pub(in crate::analysis::query) fn request(&self) -> crate::analysis::sema::MemberRequest<'_> {
        crate::analysis::sema::MemberRequest {
            site: self.site,
            offset: self.offset,
            used: &self.used,
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) struct FieldKey {
    pub(crate) colon: bool,
    pub(crate) comma: bool,
}
pub(crate) use crate::analysis::query::signatures::label::{CallableSignature, ParameterLabel};
pub(in crate::analysis::query) use signatures::SignatureSource;
pub(in crate::analysis::query) use type_position::type_position;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Struct,
    Interface,
    TypeName,
    Function,
    Method,
    Variable,
    Constant,
    Field,
}
pub(in crate::analysis::query) struct Presentation {
    pub(crate) detail: Option<String>,
    pub(crate) signature: Option<(CallableSignature, bool)>,
    pub(crate) type_parameters: Option<Vec<String>>,
    pub(crate) kind: Option<Kind>,
}

/// Domain result; protocol edit shape, snippets and markup tags belong to the
/// feature adapter. No lexical bindings or indexed declarations escape here.
pub(crate) struct Completion {
    pub(crate) span: std::ops::Range<usize>,
    pub(crate) calls: bool,
    pub(crate) items: Vec<Item>,
}

pub(crate) struct Item {
    pub(crate) label: String,
    pub(crate) kind: Kind,
    pub(crate) detail: Option<String>,
    pub(crate) signature: Option<CallableSignature>,
    pub(crate) type_parameters: Option<Vec<String>>,
    pub(crate) documentation: Option<String>,
    pub(crate) field_key: Option<FieldKey>,
}

impl<'a> Context<'a> {
    pub(crate) fn completion(
        &mut self,
        offset: usize,
        call_context: bool,
        render_documentation: Option<&mut dyn FnMut(&str) -> String>,
    ) -> Result<Completion, (i32, String)> {
        candidates::collect(self, offset, call_context, render_documentation)
    }
    pub(crate) fn trace_queries(&self) -> bool {
        self.workspace.trace.queries
    }

    pub(in crate::analysis::query) fn imported_symbols(
        &self,
        alias: &str,
    ) -> impl Iterator<Item = (&'a std::path::PathBuf, &'a crate::index::IndexedSymbol)> + use<'a>
    {
        self.resolver().imported_symbol_iter(self.file(), alias)
    }

    pub(in crate::analysis::query) fn file_symbols_in_scope(
        &self,
    ) -> impl Iterator<Item = (&'a std::path::PathBuf, &'a crate::index::IndexedSymbol)> + use<'a>
    {
        self.resolver().file_symbol_iter(self.file())
    }
}
