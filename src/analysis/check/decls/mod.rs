//! Tier 1 demand-driven declaration resolver with cycle detection (`obj_path`),
//! whole `const (...)` group batch caching, generic origin/signature caching,
//! and unified constant-expression evaluation.

mod const_eval;
mod values;

use std::collections::HashMap;
use std::path::Path;

use crate::analysis::engine::budget::{MAX_BYTES, MAX_DECL_CACHE, MAX_DEPTH};
use crate::analysis::engine::obj::DeclId;
use crate::analysis::engine::types::{Mode, Operand, TypeArena, TypeId};
use crate::syntax::{ExprId, TypeSyntax};

pub(in crate::analysis::engine) use const_eval::eval_package_const_expr;

#[derive(Clone, Copy)]
enum DeclEntry {
    Checking,
    Ready(Operand),
    GenericType(TypeId),
    InvalidVar,
    Failed,
}

/// Request-scoped resolver and cache of cross-file / cross-package declarations.
/// Only declaration header slices are read on demand; bodies are never parsed here.
pub(crate) struct DeclResolver<'a> {
    slots: HashMap<DeclId<'a>, DeclEntry>,
    obj_path: Vec<DeclId<'a>>,
}

/// Compatibility alias preserved for `analysis::sema` and `analysis::check` callers.
pub(crate) type DeclTable<'a> = DeclResolver<'a>;

impl<'a> DeclResolver<'a> {
    pub(crate) fn new() -> Self {
        Self {
            slots: HashMap::new(),
            obj_path: Vec::new(),
        }
    }

    /// Seed a resolved package-level value declaration from an open-file
    /// `RegionInfo` check so subsequent references avoid re-checking its initializer.
    pub(crate) fn seed_value(&mut self, decl: DeclId<'a>, operand: Operand) {
        if self.slots.len() >= MAX_DECL_CACHE
            || self.slots.contains_key(&decl)
            || !operand.ty.is_decl_safe()
        {
            return;
        }
        if operand.mode == Mode::Invalid {
            self.slots.insert(decl, DeclEntry::InvalidVar);
        } else if operand.mode.is_value() {
            self.slots.insert(decl, DeclEntry::Ready(operand));
        }
    }

    /// Resolve and cache the `DeclArena` origin template `TypeId` for a
    /// package-level generic type declaration without changing `operand_of`'s
    /// `None` semantics for uninstantiated generic types.
    pub(crate) fn generic_type_origin(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
    ) -> Option<TypeId> {
        if !decl.symbol.is_type() || !decl.symbol.generic {
            return None;
        }
        let file = arena
            .workspace()
            .files
            .get_key_value(decl.file)?
            .0
            .as_path();
        let decl = DeclId {
            file,
            symbol: decl.symbol,
        };
        if let Some(entry) = self.slots.get(&decl).copied() {
            return match entry {
                DeclEntry::GenericType(ty) => Some(ty),
                DeclEntry::Ready(op) if op.mode == Mode::TypeExpr => Some(op.ty),
                DeclEntry::Checking
                | DeclEntry::Ready(_)
                | DeclEntry::InvalidVar
                | DeclEntry::Failed => None,
            };
        }
        if self.slots.len() >= MAX_DECL_CACHE || self.obj_path.len() >= MAX_DEPTH {
            return None;
        }
        self.slots.insert(decl, DeclEntry::Checking);
        self.obj_path.push(decl);
        let resolved = arena.with_decl_alloc(|arena| arena.declaration_type(decl));
        self.obj_path.pop();
        match resolved {
            Some(ty) if ty.is_decl_safe() => {
                self.slots.insert(decl, DeclEntry::GenericType(ty));
                Some(ty)
            }
            Some(_) | None => {
                self.slots.insert(decl, DeclEntry::Failed);
                None
            }
        }
    }

    pub(crate) fn operand_of(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
    ) -> Option<Operand> {
        // Uninstantiated generic types do not produce a concrete `Mode::TypeExpr`
        // operand here; callers wanting their origin template use `generic_type_origin`.
        if decl.symbol.is_type() && decl.symbol.generic {
            return None;
        }
        let file = arena
            .workspace()
            .files
            .get_key_value(decl.file)?
            .0
            .as_path();
        let decl = DeclId {
            file,
            symbol: decl.symbol,
        };
        if let Some(entry) = self.slots.get(&decl).copied() {
            return match entry {
                DeclEntry::Ready(operand) => Some(operand),
                DeclEntry::Checking
                | DeclEntry::GenericType(_)
                | DeclEntry::InvalidVar
                | DeclEntry::Failed => None,
            };
        }
        if self.slots.len() >= MAX_DECL_CACHE || self.obj_path.len() >= MAX_DEPTH {
            return None;
        }
        self.slots.insert(decl, DeclEntry::Checking);
        self.obj_path.push(decl);
        let resolved = arena.with_decl_alloc(|arena| self.resolve_uncached(arena, decl));
        self.obj_path.pop();
        match resolved {
            Some(operand) if operand.mode == Mode::Invalid => {
                self.slots.insert(decl, DeclEntry::InvalidVar);
                None
            }
            Some(operand) if operand.ty.is_decl_safe() => {
                self.slots.insert(decl, DeclEntry::Ready(operand));
                Some(operand)
            }
            Some(_) | None => {
                self.slots.insert(decl, DeclEntry::Failed);
                None
            }
        }
    }

    pub(super) fn invalid_var(&self, decl: DeclId<'a>) -> bool {
        matches!(self.slots.get(&decl), Some(DeclEntry::InvalidVar))
    }

    pub(crate) fn type_of(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
    ) -> Option<TypeId> {
        self.operand_of(arena, decl).map(|op| op.ty)
    }

    fn resolve_uncached(&mut self, arena: &mut TypeArena<'a>, decl: DeclId<'a>) -> Option<Operand> {
        if decl.symbol.is_type() {
            if decl.symbol.generic {
                return None;
            }
            // unsafe.Pointer is a compiler-defined basic type; its SDK doc stub `*ArbitraryType`
            // must not be treated as a dereferenceable pointer type.
            if arena.workspace().import_path(decl.file).as_deref() == Some("unsafe") {
                if decl.symbol.name == "Pointer" {
                    let ty =
                        arena.basic(crate::analysis::engine::types::BasicKind::UnsafePointer)?;
                    return Some(Operand {
                        mode: Mode::TypeExpr,
                        ty,
                        value: None,
                    });
                }
                if matches!(decl.symbol.name.as_str(), "ArbitraryType" | "IntegerType") {
                    return None;
                }
            }
            let ty = arena.push_named(decl, &[], decl.symbol.alias)?;
            if decl.symbol.alias {
                arena.unalias(ty)?;
            }
            return Some(Operand {
                mode: Mode::TypeExpr,
                ty,
                value: None,
            });
        }
        if decl.symbol.is_function() || decl.symbol.is_method() {
            return self.resolve_signature(arena, decl);
        }
        if decl.symbol.is_const() || decl.symbol.is_var() {
            return self.resolve_value(arena, decl);
        }
        None
    }

    fn resolve_signature(
        &mut self,
        arena: &mut TypeArena<'a>,
        decl: DeclId<'a>,
    ) -> Option<Operand> {
        let workspace = arena.workspace();
        let declaration = decl.symbol.declaration()?;
        let end = decl
            .symbol
            .body()
            .map_or(declaration.end, |body| body.start);
        if end.saturating_sub(declaration.start) > MAX_BYTES {
            return None;
        }
        let header = workspace.source_slice(decl.file, declaration.start..end)?;
        let signature = crate::syntax::parse_signature(&header);
        if !signature.diagnostics.is_empty() {
            return None;
        }
        let mut const_eval = |arena: &mut TypeArena<'a>,
                              origin: &Path,
                              source: &str,
                              parsed: &TypeSyntax,
                              expr_id: ExprId| {
            self.eval_const_expr(arena, origin, source, parsed, expr_id)
        };
        let has_recv_params = signature.receiver.first().is_some_and(|receiver| {
            !crate::analysis::engine::scope::receiver_parameters(&header, &receiver.annotation)
                .is_empty()
        });
        let ty = if has_recv_params {
            arena.resolve_generic_receiver_signature_with_eval(
                decl.file,
                &header,
                Some(declaration.start),
                &signature,
                &mut const_eval,
            )?
        } else if signature.type_parameters.is_empty() {
            arena.signature_with_eval(
                decl.file,
                &header,
                Some(declaration.start),
                &signature,
                &[],
                &mut const_eval,
                0,
            )?
        } else if signature.receiver.is_empty() {
            arena.resolve_generic_signature_with_eval(
                decl.file,
                &header,
                Some(declaration.start),
                &signature,
                &mut const_eval,
            )?
        } else {
            return None;
        };
        Some(Operand {
            mode: Mode::Value,
            ty,
            value: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::engine::types::Constant;
    use crate::workspace::{self, Workspace};
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn unique_temp_dir(tag: &str) -> TempDir {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("goquery-{tag}-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn init_temp_workspace(root: &Path) -> Workspace {
        let mut workspace = Workspace::default();
        workspace
            .initialize(&json!({
                "rootUri": workspace::uri(root),
                "initializationOptions": { "goroot": "", "fileWatch": false },
            }))
            .expect("initialize workspace");
        workspace
    }

    #[test]
    fn const_group_batch_caches_iota_siblings_without_rereading_disk() {
        let dir = unique_temp_dir("const-group-batch");
        std::fs::write(dir.0.join("go.mod"), "module example.com/m\n\ngo 1.27\n")
            .expect("write go.mod");
        let file = dir.0.join("consts.go");
        let src = concat!(
            "package p\n",
            "const (\n",
            "    First = iota + 10\n",
            "    Second\n",
            "    Third\n",
            "    Fourth = Third * 2\n",
            ")\n",
        );
        std::fs::write(&file, src).expect("write consts.go");

        let workspace = init_temp_workspace(&dir.0);
        let (interned, stub) = workspace
            .files
            .iter()
            .find(|(p, _)| p.ends_with("consts.go"))
            .expect("indexed consts.go");
        let find = |name: &str| DeclId {
            file: interned.as_path(),
            symbol: stub
                .symbols
                .iter()
                .find(|s| s.name == name)
                .expect("const symbol"),
        };
        let first = find("First");
        let second = find("Second");
        let third = find("Third");
        let fourth = find("Fourth");

        let mut store = TypeArena::with_shared_budget(&workspace, workspace.build_view(None));
        let mut decls = DeclResolver::new();

        // Resolving `Third` parses the `const (...)` group once and seeds its siblings.
        let third_op = decls.operand_of(&mut store, third).expect("resolve Third");
        assert!(matches!(third_op.value, Some(Constant::Int(12))));

        // Remove the closed file from disk: all sibling lookups must hit `DeclResolver.slots`.
        std::fs::remove_file(interned).expect("remove temp file");

        let first_op = decls.operand_of(&mut store, first).expect("cached First");
        let second_op = decls.operand_of(&mut store, second).expect("cached Second");
        let fourth_op = decls.operand_of(&mut store, fourth).expect("cached Fourth");
        assert!(matches!(first_op.value, Some(Constant::Int(10))));
        assert!(matches!(second_op.value, Some(Constant::Int(11))));
        assert!(matches!(fourth_op.value, Some(Constant::Int(24))));
    }
}
