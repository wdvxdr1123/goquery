//! Lifetime-branded semantic type handles and orthogonal inspection over [`TypeArena`].
//! [`TypeId`] stays sealed inside `analysis::engine`; external callers interact exclusively
//! through [`TypeRef<'r>`], [`TypeAndValue<'r>`], and [`TypeInspector`].

use std::marker::PhantomData;
use std::path::Path;

use crate::analysis::engine::types::{
    BasicKind, Constant, Mode, Operand, Qualifier, TypeArena, TypeDeclaration, TypeForm, TypeId,
    type_form_in_scope,
};

use super::CallableView;

/// Lifetime-locked 4-byte handle to a [`TypeId`] within the active [`super::RegionView`]
/// or [`super::PackageView`]. Prevents dangling handles across `LocalArena` epoch resets
/// at compile time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TypeRef<'r> {
    pub(super) id: TypeId,
    _brand: PhantomData<&'r ()>,
}

const _: () = assert!(std::mem::size_of::<TypeRef<'static>>() == 4);

impl<'r> TypeRef<'r> {
    pub(super) fn new(id: TypeId) -> Self {
        Self {
            id,
            _brand: PhantomData,
        }
    }
}

/// Short compatibility alias for [`TypeRef`].
pub(crate) type TypeHandle<'r> = TypeRef<'r>;

/// Checked expression or binding type paired with constant mode and value.
#[derive(Clone, Copy)]
pub(crate) struct TypeAndValue<'r> {
    pub(crate) mode_is_const: bool,
    pub(crate) ty: TypeRef<'r>,
    pub(crate) value: Option<Constant>,
}

impl<'r> TypeAndValue<'r> {
    pub(super) fn from_operand(op: Operand) -> Self {
        Self {
            mode_is_const: op.mode == Mode::Constant,
            ty: TypeRef::new(op.ty),
            value: op.value,
        }
    }
}

/// Result of inspecting a type's semantic-token modifier shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TypeShape {
    Form(TypeForm),
    /// Explicitly suppressed (e.g., type parameter or untyped bool/string constant).
    Suppressed,
    Unknown,
}

/// Orthogonal, zero-`String`-allocation semantic type inspection view backed by
/// `&mut TypeArena<'a>` and branded with region lifetime `'r`.
pub(crate) struct TypeInspector<'t, 'r, 'a> {
    file: &'a Path,
    arena: &'t mut TypeArena<'a>,
    _region: PhantomData<&'r ()>,
}

impl<'t, 'r, 'a> TypeInspector<'t, 'r, 'a> {
    pub(super) fn new(file: &'a Path, arena: &'t mut TypeArena<'a>) -> Self {
        Self {
            file,
            arena,
            _region: PhantomData,
        }
    }

    /// Semantically tests whether `ty` (unaliasing aliases) is the universe `error` type,
    /// without rendering to a `String` or mutating persistent arena state.
    pub(crate) fn is_error(&mut self, ty: TypeRef<'r>) -> bool {
        let Some(unaliased) = self.arena.unalias(ty.id) else {
            return false;
        };
        if self.arena.basic_kind(unaliased).is_some()
            || self.arena.named_declaration(unaliased).is_some()
        {
            return false;
        }
        if self.is_interface(TypeRef::new(unaliased)) != Some(true) {
            return false;
        }
        let file = self.file;
        self.arena
            .with_projection(false, |arena| {
                let err = arena.builtin_type(file, "error")?;
                arena.identical(unaliased, err)
            })
            .unwrap_or(false)
    }

    pub(crate) fn is_callable(&mut self, ty: TypeRef<'r>) -> Option<bool> {
        self.arena.is_callable(ty.id)
    }

    pub(crate) fn is_interface(&mut self, ty: TypeRef<'r>) -> Option<bool> {
        self.arena.is_interface(ty.id)
    }

    pub(crate) fn is_type_param(&mut self, ty: TypeRef<'r>) -> bool {
        self.arena.is_type_param(ty.id)
    }

    pub(crate) fn is_untyped_bool_or_string(&mut self, ty: TypeRef<'r>) -> bool {
        self.arena.is_untyped_bool_or_string(ty.id)
    }

    pub(crate) fn format(&mut self, ty: TypeRef<'r>, qualifier: Qualifier<'_>) -> Option<String> {
        match qualifier {
            Qualifier::RelativeTo(consumer) => self.arena.render(ty.id, consumer),
            Qualifier::Presentation(consumer) => self.arena.render_presentation(ty.id, consumer),
            Qualifier::FullPath => self.arena.diagnostic_type(ty.id),
        }
    }

    pub(crate) fn format_for_local(
        &mut self,
        ty: TypeRef<'r>,
        file: &Path,
        qualifier: Qualifier<'_>,
    ) -> Option<String> {
        self.format(ty, qualifier).or_else(|| {
            self.arena
                .has_local_declaration(ty.id)
                .filter(|has| *has)
                .and_then(|_| self.format(ty, Qualifier::Presentation(file)))
        })
    }

    pub(crate) fn type_shape(&mut self, ty: TypeRef<'r>, constant: bool) -> TypeShape {
        if let Some(root) = self.arena.underlying(ty.id)
            && self.arena.basic_kind(root) == Some(BasicKind::UnsafePointer)
        {
            return TypeShape::Form(TypeForm::Pointer);
        }
        if self.is_type_param(ty) {
            return TypeShape::Suppressed;
        }
        let Some(form) = self.arena.form(ty.id) else {
            return TypeShape::Unknown;
        };
        if constant && self.is_untyped_bool_or_string(ty) {
            return TypeShape::Suppressed;
        }
        TypeShape::Form(form)
    }

    pub(crate) fn callable_view(
        &mut self,
        ty: TypeRef<'r>,
        consumer: &Path,
    ) -> Option<CallableView> {
        CallableView::from_type(self.arena, ty.id, consumer)
    }

    pub(crate) fn format_parameter_index(&mut self, ty: TypeRef<'r>) -> Option<usize> {
        self.arena.format_parameter(ty.id)
    }

    pub(crate) fn projected_type_declarations(
        &mut self,
        ty: TypeRef<'r>,
    ) -> Option<Result<Vec<TypeDeclaration<'a>>, (i32, String)>> {
        self.arena.projected_type_declarations(ty.id)
    }

    pub(crate) fn builtin_type(&mut self, file: &Path, name: &str) -> Option<TypeRef<'r>> {
        self.arena.builtin_type(file, name).map(TypeRef::new)
    }

    pub(crate) fn type_form_in_scope(
        &mut self,
        file: &Path,
        annotation: &str,
        is_shadowed: impl Fn(&str) -> bool,
    ) -> Option<TypeForm> {
        self.arena.reset_steps();
        type_form_in_scope(self.arena, file, annotation, is_shadowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::sema::{RegionMode, SemanticSession};
    use crate::workspace::{Workspace, WorkspaceEvent};
    use std::path::PathBuf;

    fn session_for(source: &str) -> (Workspace, PathBuf) {
        let mut ws = Workspace::default();
        ws.initialize(&serde_json::json!({
            "initializationOptions": { "goroot": "", "fileWatch": false }
        }))
        .expect("initialize workspace");
        let file = PathBuf::from("test.go");
        ws.apply(WorkspaceEvent::SetBuffer {
            file: file.clone(),
            source: source.to_owned(),
            dirty: false,
        })
        .expect("set buffer");
        (ws, file)
    }

    #[test]
    fn type_inspector_checks_error_callable_and_interface_semantically() {
        let source = concat!(
            "package p\n",
            "type ErrAlias = error\n",
            "type CustomErr struct{}\n",
            "func (CustomErr) Error() string { return \"\" }\n",
            "type Fn func(string, ...any)\n",
            "func probe(e error, ea ErrAlias, ce CustomErr, f Fn, n int) {\n",
            "\t_ = e; _ = ea; _ = ce; _ = f; _ = n\n",
            "\t{\n",
            "\t\ttype error int\n",
            "\t\tvar shadowed error\n",
            "\t\t_ = shadowed\n",
            "\t}\n",
            "}\n",
        );
        let (ws, file) = session_for(source);
        let view = ws.build_view(Some(&file));
        let mut sema = SemanticSession::new(&ws, view);
        let probe_off = source.find("_ = e").expect("probe body") as u32;
        let mut region = sema
            .region_at(&file, source, probe_off, RegionMode::Full)
            .expect("probe region");

        fn lookup_ty<'r, 'a>(
            region: &mut super::super::RegionView<'r, 'a>,
            source: &str,
            needle: &str,
        ) -> TypeRef<'r> {
            let off = source.find(needle).expect(needle) as u32 + 4;
            let (span, obj) = region
                .object_at(off)
                .unwrap_or_else(|| panic!("object at {needle} (off={off})"));
            region
                .type_of_object(obj, span)
                .unwrap_or_else(|| panic!("type_of_object at {needle}"))
        }

        let e_ty = lookup_ty(&mut region, source, "_ = e");
        let ea_ty = lookup_ty(&mut region, source, "_ = ea");
        let ce_ty = lookup_ty(&mut region, source, "_ = ce");
        let f_ty = lookup_ty(&mut region, source, "_ = f");
        let n_ty = lookup_ty(&mut region, source, "_ = n");
        let shadowed_ty = lookup_ty(&mut region, source, "_ = shadowed");

        let mut inspector = region.types();
        assert!(inspector.is_error(e_ty));
        assert!(inspector.is_error(ea_ty));
        assert!(!inspector.is_error(ce_ty));
        assert!(!inspector.is_error(n_ty));
        assert!(!inspector.is_error(shadowed_ty));

        assert_eq!(inspector.is_interface(e_ty), Some(true));
        assert_eq!(inspector.is_interface(ce_ty), Some(false));

        assert_eq!(inspector.is_callable(f_ty), Some(true));
        assert_eq!(inspector.is_callable(n_ty), Some(false));
        assert_eq!(
            inspector.type_shape(f_ty, false),
            TypeShape::Form(TypeForm::Signature)
        );
        assert_eq!(
            inspector.type_shape(n_ty, false),
            TypeShape::Form(TypeForm::Number)
        );
    }
}
