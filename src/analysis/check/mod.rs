//! Demand-driven semantic checker (`DeclTable`, `RegionInfo`, `RegionChecker`).
//! Intra-body semantic checker for Go functions and declaration initializers.

mod assign;
mod builtins;
mod call;
pub(crate) mod decls;
pub(crate) mod diagnostics;
mod expr;
mod literals;
mod range;
mod record;
pub(crate) mod region;
mod signature;
mod stmt;
mod typexpr;

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;

use crate::analysis::engine::check::decls::DeclTable;
use crate::analysis::engine::check::record::UntypedTable;
use crate::analysis::engine::check::region::{RecordMask, RegionInfo};
use crate::analysis::engine::obj::{
    BuiltinFunction, DeclId, LocalId, Object, ObjectRef, SymbolOrigin, VarKind,
};
use crate::analysis::engine::scope::tree::{LocalBinding, LocalBindingKind};
use crate::analysis::engine::types::{BasicKind, Mode, Operand, TypeArena, TypeId};
use crate::syntax::{BlockTree, SymbolKind};

/// Operands share one mode model; packages and unapplied generics retain their
/// declaration environments until they can produce a concrete operand.
pub(crate) enum ExprOutcome<'a, 'b> {
    Value(Operand),
    /// An exact missing-name lookup at this expression or its direct callee.
    MissingName,
    /// An imported variable whose declaration has a proven invalid initializer.
    InvalidImportedVar,
    Package {
        alias: &'b str,
    },
    GenericFunc {
        decl: DeclId<'a>,
        ty: TypeId,
        explicit_type_args: Vec<TypeId>,
    },
    GenericType(DeclId<'a>),
}

impl ExprOutcome<'_, '_> {
    fn operand(self) -> Option<Operand> {
        match self {
            Self::Value(op) if op.mode.is_value() || op.mode == Mode::TypeExpr => Some(op),
            _ => None,
        }
    }

    fn invalid() -> Self {
        Self::Value(Operand::non_value(Mode::Invalid))
    }

    fn no_value() -> Self {
        Self::Value(Operand::non_value(Mode::NoValue))
    }

    fn builtin(function: BuiltinFunction) -> Self {
        Self::Value(Operand::non_value(Mode::Builtin(function)))
    }
}

#[derive(Clone)]
pub(crate) struct CheckEnv<'a> {
    source_base: usize,
    active_locals: Vec<bool>,
    pub(in crate::analysis::engine::check) declaring_type: Option<(LocalId, Option<TypeId>)>,
    pub(in crate::analysis::engine::check) type_param_owner: Option<DeclId<'a>>,
    pub(in crate::analysis::engine::check) type_param_env: Vec<(String, TypeId)>,
    pub(in crate::analysis::engine::check) type_formals: Vec<(String, usize, Option<String>)>,
    recv_annotation: Option<String>,
    pub(in crate::analysis::engine::check) result_types: Vec<Option<TypeId>>,
    pub(in crate::analysis::engine::check) named_results: bool,
}

impl<'a> CheckEnv<'a> {
    fn new(binding_count: usize) -> Self {
        Self {
            source_base: 0,
            active_locals: vec![false; binding_count],
            declaring_type: None,
            type_param_owner: None,
            type_param_env: Vec::new(),
            type_formals: Vec::new(),
            recv_annotation: None,
            result_types: Vec::new(),
            named_results: false,
        }
    }

    pub(in crate::analysis::engine::check) fn is_declaring(&self, id: LocalId) -> bool {
        self.declaring_type
            .is_some_and(|(decl_id, _)| decl_id == id)
    }

    pub(in crate::analysis::engine::check) fn is_active(&self, id: LocalId) -> bool {
        self.active_locals
            .get(id.0 as usize)
            .copied()
            .unwrap_or(false)
    }
}

pub(in crate::analysis::engine::check) fn collect_visible_type_env<'s>(
    region: &'s RegionInfo<'_>,
    env: &'s CheckEnv<'_>,
    source: &'s str,
    abs_offset: usize,
) -> Vec<(&'s str, TypeId)> {
    let mut seen: BTreeSet<&'s str> = BTreeSet::new();
    let mut out = Vec::new();

    for (name, local_id) in region.scopes.visible_bindings(source, abs_offset as u32) {
        if !env.is_active(local_id) && !env.is_declaring(local_id) {
            continue;
        }
        if !seen.insert(name) {
            continue;
        }
        let Some(b) = region.scopes.bindings.get(local_id.0 as usize) else {
            continue;
        };
        match b.kind {
            LocalBindingKind::TypeDecl { .. } => {
                if let Some((decl_id, Some(ty))) = env.declaring_type
                    && decl_id == local_id
                {
                    out.push((name, ty));
                } else if let Some(op) = region.local_type(local_id) {
                    out.push((name, op.ty));
                }
            }
            LocalBindingKind::TypeParam => {
                if let Some(op) = region.local_type(local_id) {
                    out.push((name, op.ty));
                }
            }
            _ => {}
        }
    }

    for (name, ty) in &env.type_param_env {
        if seen.insert(name.as_str()) {
            out.push((name.as_str(), *ty));
        }
    }
    out
}

/// Checks a single function body or declaration initializer into [`RegionInfo`].
pub(crate) struct RegionChecker<'a, 'b> {
    pub(crate) arena: &'b mut TypeArena<'a>,
    pub(crate) decls: &'b mut DeclTable<'a>,
    pub(crate) file: &'a Path,
    pub(crate) source: &'b str,
    pub(crate) body_base: usize,
    pub(crate) body: &'b BlockTree,
    pub(crate) region: &'b mut RegionInfo<'a>,
    pub(crate) record_mask: RecordMask,
    pub(crate) iota: Option<u32>,
    pub(crate) cursor_offset: Option<usize>,
    pub(in crate::analysis::engine::check) env: CheckEnv<'a>,
    untyped: UntypedTable,
}

impl<'a, 'b> RegionChecker<'a, 'b> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        arena: &'b mut TypeArena<'a>,
        decls: &'b mut DeclTable<'a>,
        file: &'a Path,
        source: &'b str,
        body_base: usize,
        body: &'b BlockTree,
        region: &'b mut RegionInfo<'a>,
        iota: Option<u32>,
        cursor_offset: Option<usize>,
    ) -> Self {
        let binding_count = region.scopes.bindings.len();
        Self::with_env(
            arena,
            decls,
            file,
            source,
            body_base,
            body,
            region,
            iota,
            cursor_offset,
            CheckEnv::new(binding_count),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::analysis::engine::check) fn with_env(
        arena: &'b mut TypeArena<'a>,
        decls: &'b mut DeclTable<'a>,
        file: &'a Path,
        source: &'b str,
        body_base: usize,
        body: &'b BlockTree,
        region: &'b mut RegionInfo<'a>,
        iota: Option<u32>,
        cursor_offset: Option<usize>,
        env: CheckEnv<'a>,
    ) -> Self {
        region.uses_complete &= body.diagnostics().is_empty() && cursor_offset.is_none();
        let record_mask = region.mask;
        Self {
            arena,
            decls,
            file,
            source,
            body_base,
            body,
            region,
            record_mask,
            iota,
            cursor_offset,
            env,
            untyped: UntypedTable::new(),
        }
    }

    pub(super) fn set_source_base(&mut self, base: usize) {
        self.env.source_base = base;
    }

    pub(super) fn file_offset(&self, source_offset: usize) -> usize {
        self.env.source_base + source_offset
    }

    pub(crate) fn set_type_param_owner(&mut self, owner: DeclId<'a>) {
        self.env.type_param_owner = Some(owner);
    }

    pub(crate) fn arena(&mut self) -> &mut TypeArena<'a> {
        self.arena
    }

    pub(crate) fn int_bits(&mut self) -> Option<u32> {
        let bits = self.arena.integer_bits();
        self.region.uses_complete &= bits.is_some();
        bits
    }

    pub(crate) fn abs_range(&self, r: &Range<usize>) -> Range<usize> {
        (self.body_base + r.start)..(self.body_base + r.end)
    }

    pub(crate) fn abs_span(&self, r: &Range<usize>) -> [u32; 2] {
        [
            (self.body_base + r.start) as u32,
            (self.body_base + r.end) as u32,
        ]
    }

    pub(crate) fn text(&self, r: &Range<usize>) -> &'b str {
        self.source
            .get(self.body_base + r.start..self.body_base + r.end)
            .unwrap_or("")
    }

    fn is_active(&self, id: LocalId) -> bool {
        self.env
            .active_locals
            .get(id.0 as usize)
            .copied()
            .unwrap_or(false)
    }

    fn mark_active(&mut self, id: LocalId) {
        let idx = id.0 as usize;
        if idx >= self.env.active_locals.len() {
            self.env.active_locals.resize(idx + 1, false);
        }
        self.env.active_locals[idx] = true;
    }

    fn binding(&self, id: LocalId) -> Option<&LocalBinding> {
        self.region.scopes.bindings.get(id.0 as usize)
    }

    fn local_object(&self, id: LocalId) -> Option<ObjectRef<'a>> {
        let b = self.binding(id)?;
        let span = b.name_span;
        let origin = SymbolOrigin::Local {
            file: self.file,
            id,
            span,
        };
        Some(match b.kind {
            LocalBindingKind::Param => Object::Var {
                origin,
                kind: VarKind::Param,
                case_scope: None,
            },
            LocalBindingKind::NamedResult => Object::Var {
                origin,
                kind: VarKind::ResultVar,
                case_scope: None,
            },
            LocalBindingKind::Receiver => Object::Var {
                origin,
                kind: VarKind::Receiver,
                case_scope: None,
            },
            LocalBindingKind::Var
            | LocalBindingKind::ShortVar
            | LocalBindingKind::RangeKey
            | LocalBindingKind::RangeVal => Object::Var {
                origin,
                kind: VarKind::Local,
                case_scope: None,
            },
            LocalBindingKind::TypeSwitchVar => Object::Var {
                origin,
                kind: VarKind::Local,
                case_scope: self
                    .region
                    .scopes
                    .binding_case_scope(id)
                    .and_then(crate::analysis::engine::obj::CaseScope::from_span),
            },
            LocalBindingKind::Const => ObjectRef::Const(origin),
            LocalBindingKind::TypeDecl { alias } => ObjectRef::TypeName {
                origin,
                is_alias: alias,
            },
            LocalBindingKind::TypeParam => {
                let index = self
                    .region
                    .scopes
                    .bindings
                    .iter()
                    .take(id.0 as usize)
                    .filter(|other| matches!(other.kind, LocalBindingKind::TypeParam))
                    .count() as u16;
                ObjectRef::TypeParam {
                    owner: self.env.type_param_owner,
                    index,
                    span,
                }
            }
        })
    }

    fn record_local_var_entry(
        &mut self,
        id: LocalId,
        span: [u32; 2],
        kind: VarKind,
        ty: Option<TypeId>,
    ) {
        let obj = Object::Var {
            origin: SymbolOrigin::Local {
                file: self.file,
                id,
                span,
            },
            kind,
            case_scope: None,
        };
        self.record_def(span, obj);
        if let Some(ty) = ty {
            self.region.set_local_type(
                id,
                Operand {
                    mode: Mode::Variable,
                    ty,
                    value: None,
                },
            );
        }
    }

    pub(super) fn record_local_var_def(
        &mut self,
        id: LocalId,
        span: [u32; 2],
        kind: VarKind,
        ty: Option<TypeId>,
    ) {
        self.mark_active(id);
        self.record_local_var_entry(id, span, kind, ty);
    }

    pub(crate) fn check_body(&mut self) {
        let root = self.body.root();
        self.check_stmt(root);
        self.flush_untyped();
    }

    fn visible_type_env(&self, abs_offset: usize) -> ((), Vec<(String, TypeId)>) {
        let env = collect_visible_type_env(self.region, &self.env, self.source, abs_offset)
            .into_iter()
            .map(|(name, ty)| (name.to_string(), ty))
            .collect();
        ((), env)
    }

    pub(in crate::analysis::engine::check) fn resolve_pkg_qualified_type_use(
        &mut self,
        qual: &str,
        name: &str,
        abs_offset: usize,
        qual_r: &Range<usize>,
        name_r: &Range<usize>,
        check_locals: bool,
    ) -> bool {
        if check_locals
            && let Some(local_id) =
                self.region
                    .scopes
                    .resolve_at(self.source, (abs_offset + qual_r.start) as u32, qual)
            && self.is_active(local_id)
        {
            if let Some(obj) = self.local_object(local_id) {
                let qual_span = [
                    (abs_offset + qual_r.start) as u32,
                    (abs_offset + qual_r.end) as u32,
                ];
                self.record_use(qual_span, obj);
            }
            return false;
        }
        let Some((pkg_obj, _target_dir)) = self.find_import(qual) else {
            self.report_missing_name(qual, abs_offset + qual_r.start..abs_offset + qual_r.end);
            return false;
        };
        let qual_span = [
            (abs_offset + qual_r.start) as u32,
            (abs_offset + qual_r.end) as u32,
        ];
        self.record_use(qual_span, pkg_obj);

        let Some(decl) = self.resolve_imported_decl(qual, name) else {
            self.report_missing_imported_name(
                qual,
                name,
                abs_offset + qual_r.start,
                abs_offset + name_r.start..abs_offset + name_r.end,
            );
            return false;
        };
        if decl.symbol.kind != SymbolKind::Type {
            return false;
        }
        let name_span = [
            (abs_offset + name_r.start) as u32,
            (abs_offset + name_r.end) as u32,
        ];
        self.record_use(name_span, decl_object(decl));
        if decl.symbol.generic {
            true
        } else {
            self.decls.type_of(self.arena, decl).is_some()
        }
    }

    pub(in crate::analysis::engine::check) fn resolve_unqualified_type_use(
        &mut self,
        name: &str,
        abs_offset: usize,
        name_r: &Range<usize>,
        check_locals: bool,
        allow_comparable: bool,
    ) -> bool {
        let name_span = [
            (abs_offset + name_r.start) as u32,
            (abs_offset + name_r.end) as u32,
        ];
        if check_locals
            && let Some(local_id) = self
                .region
                .scopes
                .resolve_at(self.source, name_span[0], name)
        {
            let active = self.is_active(local_id) || self.env.is_declaring(local_id);
            if active {
                let Some(b) = self.binding(local_id) else {
                    return false;
                };
                match b.kind {
                    LocalBindingKind::TypeDecl { .. } => {
                        let op = self.region.local_type(local_id);
                        if let Some(obj) = self.local_object(local_id) {
                            self.record_use(name_span, obj);
                        }
                        return op.is_some() || self.env.is_declaring(local_id);
                    }
                    LocalBindingKind::TypeParam => {
                        let op = self.region.local_type(local_id);
                        if let Some(obj) = self.local_object(local_id) {
                            self.record_use(name_span, obj);
                        }
                        return op.is_some();
                    }
                    _ => {
                        // go/types records the use of a local even when it is
                        // rejected in type position.
                        if let Some(obj) = self.local_object(local_id) {
                            self.record_use(name_span, obj);
                        }
                        return false;
                    }
                }
            }
        }

        if check_locals && self.record_formal_type_param_use(name, name_span) {
            return true;
        }

        if self.find_import(name).is_some() {
            return false;
        }

        match self.resolve_file_or_package_symbol(name) {
            FileSymbolLookup::Found(decl) => {
                if decl.symbol.kind != SymbolKind::Type {
                    return false;
                }
                self.record_use(name_span, decl_object(decl));
                if decl.symbol.generic {
                    true
                } else {
                    self.decls.type_of(self.arena, decl).is_some()
                }
            }
            FileSymbolLookup::Ambiguous => false,
            FileSymbolLookup::NotFound => {
                if allow_comparable && name == "comparable" {
                    self.record_use(name_span, ObjectRef::BuiltinType("comparable"));
                    true
                } else if self.arena.builtin_type(self.file, name).is_some() {
                    if let Some(builtin_name) = builtin_type_static_name(name) {
                        self.record_use(name_span, ObjectRef::BuiltinType(builtin_name));
                    }
                    true
                } else {
                    self.report_missing_name(name, name_span[0] as usize..name_span[1] as usize);
                    false
                }
            }
        }
    }

    pub(crate) fn find_import(&mut self, alias: &str) -> Option<(ObjectRef<'a>, &'a Path)> {
        let ws = self.arena.workspace();
        let (import_path_span, target_dir) =
            self.arena.env_cache.find_import(ws, self.file, alias)?;
        Some((
            ObjectRef::PkgName {
                import_path_span,
                target_dir,
            },
            target_dir,
        ))
    }

    pub(crate) fn resolve_file_or_package_symbol(&mut self, name: &str) -> FileSymbolLookup<'a> {
        let ws = self.arena.workspace();
        match self
            .arena
            .env_cache
            .resolve_unqualified(ws, self.file, name)
        {
            crate::analysis::engine::resolve::UnqualifiedLookup::Found(p, symbol) => {
                FileSymbolLookup::Found(DeclId { file: p, symbol })
            }
            crate::analysis::engine::resolve::UnqualifiedLookup::Ambiguous => {
                FileSymbolLookup::Ambiguous
            }
            crate::analysis::engine::resolve::UnqualifiedLookup::NotFound => {
                FileSymbolLookup::NotFound
            }
        }
    }

    pub(in crate::analysis::engine::check) fn resolve_imported_decl(
        &mut self,
        alias: &str,
        name: &str,
    ) -> Option<DeclId<'a>> {
        let ws = self.arena.workspace();
        self.arena
            .env_cache
            .resolve_imported(ws, self.file, alias, name)
    }

    pub(in crate::analysis::engine::check) fn record_formal_type_param_use(
        &mut self,
        name: &str,
        name_span: [u32; 2],
    ) -> bool {
        let Some((index, (_, start, _))) = self
            .env
            .type_formals
            .iter()
            .enumerate()
            .find(|(_, (n, _, _))| n == name)
        else {
            return false;
        };
        let decl_span = [*start as u32, (*start + name.len()) as u32];
        if name_span != decl_span {
            self.record_use(
                name_span,
                ObjectRef::TypeParam {
                    owner: self.env.type_param_owner,
                    index: index as u16,
                    span: decl_span,
                },
            );
        }
        true
    }

    fn basic_bool(&mut self) -> Option<TypeId> {
        self.arena.basic(BasicKind::Bool)
    }
}

pub(crate) use crate::analysis::engine::obj::decl_object;

pub(crate) enum FileSymbolLookup<'a> {
    Found(DeclId<'a>),
    Ambiguous,
    NotFound,
}

pub(crate) fn builtin_type_static_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "any" => "any",
        "bool" => "bool",
        "byte" => "byte",
        "comparable" => "comparable",
        "complex64" => "complex64",
        "complex128" => "complex128",
        "error" => "error",
        "float32" => "float32",
        "float64" => "float64",
        "int" => "int",
        "int8" => "int8",
        "int16" => "int16",
        "int32" => "int32",
        "int64" => "int64",
        "rune" => "rune",
        "string" => "string",
        "uint" => "uint",
        "uint8" => "uint8",
        "uint16" => "uint16",
        "uint32" => "uint32",
        "uint64" => "uint64",
        "uintptr" => "uintptr",
        _ => return None,
    })
}
