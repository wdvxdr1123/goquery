//! Lifetime-branded semantic type handles and orthogonal inspection over [`TypeArena`].
//! [`TypeId`] stays sealed inside `analysis::engine`; external callers interact exclusively
//! through [`TypeRef<'r>`], [`TypeAndValue<'r>`], and [`TypeInspector`].

use std::marker::PhantomData;
use std::path::Path;

use crate::analysis::engine::types::{
    BasicKind, Constant, Mode, Operand, Qualifier, TypeArena, TypeDeclaration, TypeForm, TypeId,
    type_form_in_scope,
};

use super::{CallableView, Layout, StructLayout};

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
        if !ty.id.is_decl() && !ty.id.is_local() {
            return false;
        }
        let Some(unaliased) = self.arena.unalias(ty.id) else {
            return false;
        };
        if (!unaliased.is_decl() && !unaliased.is_local())
            || self.arena.named_declaration(unaliased).is_some()
            || self.arena.local_type_annotation(unaliased).is_some()
        {
            return false;
        }
        // Only `TypeOrigin::BuiltinError` (non-package, non-local named type) has
        // a distinct underlying interface; structural types, anonymous interfaces
        // (`any`, `interface{...}`), and type parameters satisfy `underlying == unaliased`.
        if self.arena.underlying(unaliased) == Some(unaliased) {
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

    #[allow(dead_code)]
    pub(crate) fn layout(&mut self, ty: TypeRef<'r>) -> Option<Layout> {
        self.arena.layout(ty.id)
    }

    pub(crate) fn struct_layout(&mut self, ty: TypeRef<'r>) -> Option<StructLayout> {
        self.arena.struct_layout(ty.id)
    }

    pub(crate) fn is_callable(&mut self, ty: TypeRef<'r>) -> Option<bool> {
        self.arena.is_callable(ty.id)
    }

    #[allow(dead_code)]
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
