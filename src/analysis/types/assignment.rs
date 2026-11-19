//! Go assignment relations over values, independent of their source locations.
use super::{BasicKind, CheckError, ConstAssignError, Direction, Operand, Ty, TypeArena, TypeId};
use CheckError::{Invalid, Unknown};

#[derive(Clone, Copy)]
pub(crate) enum AssignmentError {
    TypeMismatch,
    InterfaceMismatch,
    ImplicitConstant(ConstAssignError),
}

impl TypeArena<'_> {
    pub(crate) fn assignable(
        &mut self,
        source: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<bool> {
        match self.check_assignment(source, target, int_bits) {
            Ok(()) => Some(true),
            Err(Invalid(_)) => Some(false),
            Err(Unknown) => None,
        }
    }

    pub(crate) fn check_assignment(
        &mut self,
        source: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Result<(), CheckError<AssignmentError>> {
        self.step().ok_or(Unknown)?;
        let mismatch = Invalid(AssignmentError::TypeMismatch);
        if !source.mode.is_value() {
            return Err(mismatch);
        }
        // Compare declaration identities before expanding types. Interface
        // constructors still need their complete basic method set.
        if !self.interface_constructor(target).ok_or(Unknown)?
            && self.identical(source.ty, target).ok_or(Unknown)?
        {
            return Ok(());
        }
        if self.is_interface(target).ok_or(Unknown)? {
            return self
                .assign_interface(source, target, int_bits)
                .ok_or(Unknown)?
                .then_some(())
                .ok_or(Invalid(AssignmentError::InterfaceMismatch));
        }
        if let Some(kind) = self.basic_kind(source.ty).filter(|kind| kind.untyped()) {
            if kind == BasicKind::UntypedNil {
                return self
                    .nilable(target)
                    .ok_or(Unknown)?
                    .then_some(())
                    .ok_or(mismatch);
            }
            if self.assign_basic(source, target, int_bits).is_some() {
                return Ok(());
            }
            let reason = self
                .const_assignment_error(source, target, int_bits)
                .ok_or(Unknown)?;
            return Err(Invalid(AssignmentError::ImplicitConstant(reason)));
        }
        if self.identical(source.ty, target).ok_or(Unknown)? {
            return Ok(());
        }
        let (source, target) = (
            self.unalias(source.ty).ok_or(Unknown)?,
            self.unalias(target).ok_or(Unknown)?,
        );
        if self.is_defined_type(source) && self.is_defined_type(target) {
            return Err(mismatch);
        }
        let (source, target) = (
            self.underlying(source).ok_or(Unknown)?,
            self.underlying(target).ok_or(Unknown)?,
        );
        if self.identical(source, target).ok_or(Unknown)? {
            return Ok(());
        }
        if let (
            Ty::Chan {
                direction: Direction::Both,
                element: source,
            },
            Ty::Chan {
                element: target, ..
            },
        ) = (
            &self.nodes[usize::from(source.0)],
            &self.nodes[usize::from(target.0)],
        ) {
            return self
                .identical(*source, *target)
                .ok_or(Unknown)?
                .then_some(())
                .ok_or(mismatch);
        }
        Err(mismatch)
    }
}
