//! Value-producing builtins share native identity, assignment and index rules.
use super::{BasicKind, Constant, Mode, Operand, Ty, TypeArena, TypeId};

impl TypeArena<'_> {
    pub(crate) fn builtin_length(
        &mut self,
        input: Operand,
        capacity: bool,
        has_call: bool,
        int_bits: u32,
    ) -> Option<Operand> {
        self.step()?;
        let root = self.underlying(input.ty)?;
        if matches!(self.nodes[usize::from(root.0)], Ty::Parameter(_)) {
            let terms = self.parameter_set(root)?.terms?;
            if terms.is_empty() {
                return None;
            }
            for term in terms {
                self.step()?;
                self.length_type(term.ty, capacity)?;
            }
            return self.int_result(None, int_bits);
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
                return None;
            };
            length = Some(value);
        }
        self.int_result(length, int_bits)
    }

    /// A static array length, or a valid runtime-sized len/cap operand.
    fn length_type(&mut self, input: TypeId, capacity: bool) -> Option<Option<u64>> {
        let mut root = self.underlying(input)?;
        if let Ty::Pointer(element) = self.nodes[usize::from(root.0)] {
            root = self.underlying(element)?;
            if !matches!(self.nodes[usize::from(root.0)], Ty::Array { .. }) {
                return None;
            }
        }
        match self.nodes[usize::from(root.0)] {
            Ty::Array { length, .. } => Some(Some(length)),
            Ty::Slice(_) | Ty::Chan { .. } => Some(None),
            Ty::Map { key, .. } if !capacity => self.comparable(key, 0)?.then_some(None),
            Ty::Basic(kind) if kind.string() && !capacity => Some(None),
            _ => None,
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
        sizes: &[Operand],
        int_bits: u32,
    ) -> Option<Operand> {
        self.step()?;
        self.constructor_map_keys(ty)?;
        let root = self.common_under(ty)?;
        let valid = match self.nodes[usize::from(root.0)] {
            Ty::Slice(_) => matches!(sizes.len(), 1 | 2),
            Ty::Map { key, .. } => sizes.len() <= 1 && self.comparable(key, 0)?,
            Ty::Chan { .. } => sizes.len() <= 1,
            _ => false,
        };
        if !valid {
            return None;
        }
        let mut previous = None;
        for size in sizes {
            let value = self.index_value(*size, int_bits)?;
            if previous
                .zip(value)
                .is_some_and(|(length, capacity)| length > capacity)
            {
                return None;
            }
            previous = value;
        }
        Some(Operand {
            mode: Mode::Value,
            ty,
            value: None,
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
            if matches!(self.nodes[usize::from(ty.0)], Ty::Named { .. }) {
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
    ) -> Option<Operand> {
        self.step()?;
        let input = *arguments.first()?;
        let element = self.slice_element(input.ty)?;
        if spread {
            if arguments.len() != 2 {
                return None;
            }
            if !self.byte_string_source(input, arguments[1])? {
                let source = arguments[1];
                let root = self.underlying(source.ty)?;
                if matches!(self.nodes[usize::from(root.0)], Ty::Parameter(_)) {
                    let source_element = self.slice_element(source.ty)?;
                    self.identical(element, source_element)?.then_some(())?;
                } else {
                    let target = self.push(Ty::Slice(element))?;
                    self.assignable(source, target, int_bits)?.then_some(())?;
                }
            }
        } else {
            for argument in &arguments[1..] {
                self.step()?;
                self.assignable(*argument, element, int_bits)?
                    .then_some(())?;
            }
        }
        Some(Operand {
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
    ) -> Option<Operand> {
        self.step()?;
        let destination_element = self.slice_element(destination.ty)?;
        if !self.byte_string_source(destination, source)? {
            let source_element = self.slice_element(source.ty)?;
            self.identical(destination_element, source_element)?
                .then_some(())?;
        }
        self.int_result(None, int_bits)
    }

    fn slice_element(&mut self, input: TypeId) -> Option<TypeId> {
        let root = self.common_under(input)?;
        match self.nodes[usize::from(root.0)] {
            Ty::Slice(element) => Some(element),
            _ => None,
        }
    }

    /// The string/[]byte type-set exception shared by append and copy.
    fn byte_string_source(&mut self, destination: Operand, source: Operand) -> Option<bool> {
        let element = self.slice_element(destination.ty)?;
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
}
