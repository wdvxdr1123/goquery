//! Value-producing builtins share native identity, assignment and index rules.
use super::index::IndexValueError;
use super::{
    AssignmentError, BasicKind, CheckError, Constant, Direction, Mode, Operand, Ty, TypeArena,
    TypeId,
};
use crate::analysis::obj::BuiltinFunction;
use CheckError::{Invalid, Unknown};

pub(crate) enum MakeError {
    NotMakeable,
    Count { minimum: usize },
}

pub(crate) struct MakeResult {
    pub(crate) operand: Option<Operand>,
    pub(crate) size_errors: [Option<IndexValueError>; 2],
    pub(crate) swapped: bool,
}

pub(crate) enum AppendError {
    NotSlice,
    SpreadCount {
        trailing_slice: TypeId,
    },
    Argument {
        index: usize,
        target: TypeId,
        reason: AssignmentError,
    },
}

pub(crate) enum CopyError {
    NotSlice(usize),
    DifferentElements(TypeId, TypeId),
}

pub(crate) enum ProcedureError {
    NotClearable,
    NotChannel,
    ReceiveOnly,
    NotMap,
    DifferentKeys,
}

impl TypeArena<'_> {
    pub(crate) fn builtin_length(
        &mut self,
        input: Operand,
        capacity: bool,
        has_call: bool,
        int_bits: u32,
    ) -> Option<Operand> {
        self.checked_builtin_length(input, capacity, has_call, int_bits)
            .ok()
    }

    pub(crate) fn checked_builtin_length(
        &mut self,
        input: Operand,
        capacity: bool,
        has_call: bool,
        int_bits: u32,
    ) -> Result<Operand, CheckError<()>> {
        self.step().ok_or(Unknown)?;
        let root = self.underlying(input.ty).ok_or(Unknown)?;
        if matches!(self.nodes[usize::from(root.0)], Ty::Parameter(_)) {
            let terms = self
                .parameter_set(root)
                .ok_or(Unknown)?
                .terms
                .ok_or(Unknown)?;
            if terms.is_empty() {
                return Err(Unknown);
            }
            for term in terms {
                self.step().ok_or(Unknown)?;
                self.length_type(term.ty, capacity)?;
            }
            return self.int_result(None, int_bits).ok_or(Unknown);
        }
        let mut length = self.length_type(root, capacity)?;
        if has_call {
            length = None;
        }
        if !capacity
            && self.basic_kind(root).is_some_and(BasicKind::string)
            && input.mode == Mode::Constant
        {
            let Some(Constant::String {
                length: Some(value),
            }) = input.value
            else {
                return Err(Unknown);
            };
            length = Some(value);
        }
        self.int_result(length, int_bits).ok_or(Unknown)
    }

    /// A static array length, or a valid runtime-sized len/cap operand.
    fn length_type(
        &mut self,
        input: TypeId,
        capacity: bool,
    ) -> Result<Option<u64>, CheckError<()>> {
        let mut root = self.underlying(input).ok_or(Unknown)?;
        if let Ty::Pointer(element) = self.nodes[usize::from(root.0)] {
            root = self.underlying(element).ok_or(Unknown)?;
            if !matches!(self.nodes[usize::from(root.0)], Ty::Array { .. }) {
                return Err(Invalid(()));
            }
        }
        match self.nodes[usize::from(root.0)] {
            Ty::Array { length, .. } => Ok(Some(length)),
            Ty::Slice(_) | Ty::Chan { .. } => Ok(None),
            Ty::Map { key, .. } if !capacity => {
                self.comparable(key, 0)
                    .filter(|valid| *valid)
                    .ok_or(Unknown)?;
                Ok(None)
            }
            Ty::Basic(kind) if kind.string() && !capacity => Ok(None),
            _ => Err(Invalid(())),
        }
    }

    fn int_result(&mut self, length: Option<u64>, int_bits: u32) -> Option<Operand> {
        let value = length.map(|value| Constant::Int(i128::from(value)));
        if let Some(value) = value {
            super::operand::representable(value, BasicKind::Int, int_bits).then_some(())?;
        }
        Some(Operand {
            mode: if value.is_some() {
                Mode::Constant
            } else {
                Mode::Value
            },
            ty: self.basic(BasicKind::Int)?,
            value,
        })
    }

    pub(crate) fn builtin_make(
        &mut self,
        ty: TypeId,
        sizes: [Operand; 2],
        size_count: usize,
        int_bits: u32,
    ) -> Result<MakeResult, CheckError<MakeError>> {
        self.step().ok_or(Unknown)?;
        self.constructor_map_keys(ty).ok_or(Unknown)?;
        let root = self.common_under(ty).ok_or(Unknown)?;
        let minimum = match self.nodes[usize::from(root.0)] {
            Ty::Slice(_) => 1,
            Ty::Map { key, .. } => {
                self.comparable(key, 0)
                    .filter(|valid| *valid)
                    .ok_or(Unknown)?;
                0
            }
            Ty::Chan { .. } => 0,
            _ => return Err(Invalid(MakeError::NotMakeable)),
        };
        if !(minimum..=minimum + 1).contains(&size_count) {
            return Err(Invalid(MakeError::Count {
                minimum: minimum + 1,
            }));
        }
        let mut sizes_known = true;
        let mut size_errors = [None; 2];
        let mut values = [None; 2];
        for (index, size) in sizes.into_iter().take(size_count).enumerate() {
            match self.checked_index_value(size, int_bits) {
                Ok(value) => values[index] = value,
                Err(Invalid(error)) => size_errors[index] = Some(error),
                Err(Unknown) => sizes_known = false,
            }
        }
        Ok(MakeResult {
            operand: sizes_known.then_some(Operand {
                mode: Mode::Value,
                ty,
                value: None,
            }),
            size_errors,
            swapped: values[0]
                .zip(values[1])
                .is_some_and(|(length, capacity)| length > capacity),
        })
    }

    pub(crate) fn builtin_new(&mut self, ty: TypeId) -> Option<Operand> {
        self.constructor_map_keys(ty)?;
        Some(Operand {
            mode: Mode::Value,
            ty: self.pointer(ty)?,
            value: None,
        })
    }

    /// Validate written constructor keys after local formal facts are ready.
    /// Named declarations remain lazy: allocation must not traverse their full
    /// recursive field graph. Complete declaration legality is a separate check.
    pub(crate) fn constructor_map_keys(&mut self, input: TypeId) -> Option<()> {
        let mut pending = vec![(input, 0)];
        let mut seen = Vec::new();
        while let Some((ty, depth)) = pending.pop() {
            self.step()?;
            if depth >= super::MAX_DEPTH || seen.len() >= super::MAX_NODES {
                return None;
            }
            if matches!(
                self.nodes[usize::from(ty.0)],
                Ty::Named { .. } | Ty::Alias { .. }
            ) {
                self.check_named_arguments(ty)?;
                continue;
            }
            let root = self.underlying(ty)?;
            if seen.contains(&root) {
                continue;
            }
            seen.push(root);
            let children: Vec<_> = match self.nodes[usize::from(root.0)].clone() {
                Ty::Map { key, value } => {
                    self.comparable(key, 0)?.then_some(())?;
                    vec![key, value]
                }
                Ty::Error(element)
                | Ty::Pointer(element)
                | Ty::Slice(element)
                | Ty::Array { element, .. }
                | Ty::Chan { element, .. } => vec![element],
                Ty::Tuple(elements) => self.arguments[elements.range()].to_vec(),
                Ty::Struct { fields, .. } => {
                    self.fields[fields].iter().map(|field| field.ty).collect()
                }
                Ty::Signature {
                    parameters,
                    results,
                    ..
                } => parameters
                    .chain(results)
                    .map(|index| self.arguments[index])
                    .collect(),
                Ty::Interface { methods, .. } => self.methods[methods]
                    .iter()
                    .map(|method| method.signature)
                    .collect(),
                Ty::Basic(_) | Ty::Parameter(_) => Vec::new(),
                _ => return None,
            };
            if pending.len().checked_add(children.len())? > super::MAX_NODES {
                return None;
            }
            pending.extend(children.into_iter().map(|ty| (ty, depth + 1)));
        }
        Some(())
    }

    pub(crate) fn builtin_append(
        &mut self,
        arguments: &[Operand],
        spread: bool,
        int_bits: u32,
    ) -> Result<Operand, CheckError<AppendError>> {
        self.step().ok_or(Unknown)?;
        let input = *arguments.first().ok_or(Unknown)?;
        let element = self
            .checked_slice_element(input.ty)
            .map_err(|error| error.map(|()| AppendError::NotSlice))?;
        if spread {
            if arguments.len() != 2 {
                let trailing_slice = self.slice_type(element).ok_or(Unknown)?;
                return Err(Invalid(AppendError::SpreadCount { trailing_slice }));
            }
            if !self
                .byte_string_source(element, arguments[1])
                .ok_or(Unknown)?
            {
                let source = arguments[1];
                let root = self.underlying(source.ty).ok_or(Unknown)?;
                if matches!(self.nodes[usize::from(root.0)], Ty::Parameter(_)) {
                    let source_element =
                        self.checked_slice_element(source.ty).ok().ok_or(Unknown)?;
                    self.identical(element, source_element)
                        .filter(|valid| *valid)
                        .ok_or(Unknown)?;
                } else {
                    let target = self.push(Ty::Slice(element)).ok_or(Unknown)?;
                    self.check_assignment(source, target, int_bits)
                        .map_err(|error| {
                            error.map(|reason| AppendError::Argument {
                                index: 1,
                                target,
                                reason,
                            })
                        })?;
                }
            }
        } else {
            for (index, &argument) in arguments.iter().enumerate().skip(1) {
                self.step().ok_or(Unknown)?;
                self.check_assignment(argument, element, int_bits)
                    .map_err(|error| {
                        error.map(|reason| AppendError::Argument {
                            index,
                            target: element,
                            reason,
                        })
                    })?;
            }
        }
        Ok(Operand {
            mode: Mode::Value,
            ty: input.ty,
            value: None,
        })
    }

    pub(crate) fn builtin_copy(
        &mut self,
        destination: Operand,
        source: Operand,
        int_bits: u32,
    ) -> Result<Operand, CheckError<CopyError>> {
        self.step().ok_or(Unknown)?;
        let destination_element = self
            .checked_slice_element(destination.ty)
            .map_err(|error| error.map(|()| CopyError::NotSlice(0)))?;
        if !self
            .byte_string_source(destination_element, source)
            .ok_or(Unknown)?
        {
            let source_element = match self.checked_slice_element(source.ty) {
                Ok(element) => element,
                Err(Invalid(())) if self.basic_kind(source.ty).is_some_and(BasicKind::string) => {
                    self.basic(BasicKind::Byte).ok_or(Unknown)?
                }
                Err(Invalid(())) => return Err(Invalid(CopyError::NotSlice(1))),
                Err(Unknown) => return Err(Unknown),
            };
            if !self
                .identical(destination_element, source_element)
                .ok_or(Unknown)?
            {
                return Err(Invalid(CopyError::DifferentElements(
                    destination_element,
                    source_element,
                )));
            }
        }
        self.int_result(None, int_bits).ok_or(Unknown)
    }

    fn checked_slice_element(&mut self, input: TypeId) -> Result<TypeId, CheckError<()>> {
        let root = self.common_under(input).ok_or(Unknown)?;
        match self.nodes[usize::from(root.0)] {
            Ty::Slice(element) => Ok(element),
            _ => Err(Invalid(())),
        }
    }

    /// The string/[]byte type-set exception shared by append and copy.
    fn byte_string_source(&mut self, element: TypeId, source: Operand) -> Option<bool> {
        let byte = self.basic(BasicKind::Uint8)?;
        if !self.identical(element, byte)? {
            return Some(false);
        }
        let root = self.underlying(source.ty)?;
        let terms = if matches!(self.nodes[usize::from(root.0)], Ty::Parameter(_)) {
            self.parameter_set(root)?
                .terms?
                .into_iter()
                .map(|term| term.ty)
                .collect()
        } else {
            vec![root]
        };
        if terms.is_empty() {
            return None;
        }
        for ty in terms {
            self.step()?;
            let root = self.underlying(ty)?;
            match self.nodes[usize::from(root.0)] {
                Ty::Basic(kind) if kind.string() => {}
                Ty::Slice(element) => {
                    if !self.identical(element, byte)? {
                        return Some(false);
                    }
                }
                _ => return Some(false),
            }
        }
        Some(true)
    }

    /// Validate structural operands for builtins without a result. Delete
    /// returns the common key so the caller can use the ordinary assignment check.
    pub(crate) fn builtin_procedure(
        &mut self,
        function: BuiltinFunction,
        input: TypeId,
    ) -> Result<Option<TypeId>, CheckError<ProcedureError>> {
        let root = self.underlying(input).ok_or(Unknown)?;
        let mut key = None;
        if matches!(self.nodes[usize::from(root.0)], Ty::Parameter(_)) {
            let terms = self
                .parameter_set(root)
                .ok_or(Unknown)?
                .terms
                .ok_or(Unknown)?;
            if terms.is_empty() {
                return Err(Unknown);
            }
            for term in terms {
                self.step().ok_or(Unknown)?;
                let root = self.underlying(term.ty).ok_or(Unknown)?;
                self.procedure_term(function, root, &mut key)?;
            }
        } else {
            self.procedure_term(function, root, &mut key)?;
        }
        Ok(key)
    }

    fn procedure_term(
        &mut self,
        function: BuiltinFunction,
        root: TypeId,
        key: &mut Option<TypeId>,
    ) -> Result<(), CheckError<ProcedureError>> {
        match (function, &self.nodes[usize::from(root.0)]) {
            (BuiltinFunction::Clear, Ty::Map { .. } | Ty::Slice(_)) => {}
            (
                BuiltinFunction::Close,
                Ty::Chan {
                    direction: Direction::Receive,
                    ..
                },
            ) => {
                return Err(Invalid(ProcedureError::ReceiveOnly));
            }
            (BuiltinFunction::Close, Ty::Chan { .. }) => {}
            (BuiltinFunction::Delete, Ty::Map { key: actual, .. }) => {
                let actual = *actual;
                if self.comparable(actual, 0) != Some(true) {
                    return Err(Unknown);
                }
                if let Some(previous) = *key
                    && !self.identical(previous, actual).ok_or(Unknown)?
                {
                    return Err(Invalid(ProcedureError::DifferentKeys));
                }
                *key = Some(actual);
            }
            (BuiltinFunction::Clear, _) => return Err(Invalid(ProcedureError::NotClearable)),
            (BuiltinFunction::Close, _) => return Err(Invalid(ProcedureError::NotChannel)),
            (BuiltinFunction::Delete, _) => return Err(Invalid(ProcedureError::NotMap)),
            _ => return Err(Unknown),
        }
        Ok(())
    }
}
