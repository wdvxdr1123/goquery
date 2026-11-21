//! Missing-name errors are emitted at native expression/type reference sites.
use super::{Code, Diagnostic, Kind};
use crate::analysis::engine::check::region::RecordMask;
use crate::analysis::engine::check::{FileSymbolLookup, RegionChecker};
use crate::analysis::engine::scope::universe;
use std::ops::Range;

impl RegionChecker<'_, '_> {
    /// Absence of a type/value is not absence of its declaration. Only an exact
    /// negative lookup may report a missing name; budgets and invalid types
    /// leave the fact unknown.
    pub(in crate::analysis::engine::check) fn report_missing_name(
        &mut self,
        name: &str,
        span: Range<usize>,
    ) -> bool {
        if !self.record_mask.contains(RecordMask::DIAGNOSTICS) {
            return false;
        }
        if name == "_"
            || name.len() > 1024
            || self.source.get(span.clone()) != Some(name)
            || universe::builtin(name)
            || self
                .region
                .scopes
                .resolve_at(self.source, span.start as u32, name)
                .is_some()
            || self
                .env
                .type_formals
                .iter()
                .any(|(formal, _, _)| formal == name)
            || self
                .env
                .type_param_env
                .iter()
                .any(|(formal, _)| formal == name)
            || !matches!(
                self.resolve_file_or_package_symbol(name),
                FileSymbolLookup::NotFound
            )
        {
            return false;
        }
        let workspace = self.arena.workspace();
        let Some(stub) = workspace.files.get(self.file) else {
            return false;
        };
        if stub
            .imports
            .iter()
            .any(|import| import.is_dot() || import.path == "C")
            || self.arena.resolver().imported_name(self.file, name)
        {
            return false;
        }
        self.region.push_diagnostic(Diagnostic {
            span,
            kind: Kind::Compiler(Code::UndeclaredName),
            message: format!("undefined: {name}"),
            unnecessary: false,
            related: Vec::new(),
        });
        true
    }

    /// Imported metadata must prove a unique visible package and an absent
    /// exported declaration. An unavailable dependency proves neither.
    pub(in crate::analysis::engine::check) fn report_missing_imported_name(
        &mut self,
        qualifier: &str,
        name: &str,
        qualifier_start: usize,
        span: Range<usize>,
    ) -> bool {
        if !self.record_mask.contains(RecordMask::DIAGNOSTICS) {
            return false;
        }
        if name.len() + qualifier.len() > 1024
            || self.source.get(span.clone()) != Some(name)
            || !crate::syntax::is_exported_name(name)
            || self
                .region
                .scopes
                .resolve_at(self.source, qualifier_start as u32, qualifier)
                .is_some()
            || self
                .env
                .type_formals
                .iter()
                .any(|(formal, _, _)| formal == qualifier)
            || self
                .env
                .type_param_env
                .iter()
                .any(|(formal, _)| formal == qualifier)
            || !matches!(
                self.resolve_file_or_package_symbol(qualifier),
                FileSymbolLookup::NotFound
            )
        {
            return false;
        }
        let workspace = self.arena.workspace();
        let Some(stub) = workspace.files.get(self.file) else {
            return false;
        };
        if stub
            .imports
            .iter()
            .any(|import| import.is_dot() || import.path == "C")
        {
            return false;
        }
        let mut imports = stub.imports.iter().filter(|import| {
            !import.is_dot()
                && !import.is_blank()
                && self.arena.resolver().import_name(self.file, import) == Some(qualifier)
        });
        let Some(import) = imports.next() else {
            return false;
        };
        if imports.next().is_some()
            || self
                .arena
                .resolver()
                .declared_package_name(self.file, &import.path)
                .is_none()
            || self
                .arena
                .resolver()
                .imported_symbol_iter(self.file, qualifier)
                .any(|(_, symbol)| symbol.name == name)
        {
            return false;
        }
        self.region.push_diagnostic(Diagnostic {
            span,
            kind: Kind::Compiler(Code::UndeclaredImportedName),
            message: format!("undefined: {qualifier}.{name}"),
            unnecessary: false,
            related: Vec::new(),
        });
        true
    }
}
