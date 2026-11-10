//! Range declarations derive iteration types without a consumer text round trip.
use super::{BasicKind, Direction, Mode, Operand, Ty, TypeArena, TypeId};

impl TypeArena<'_> {
    pub(crate) fn receive_slot(
        &mut self,
        input: Operand,
        result: usize,
        count: usize,
    ) -> Option<Operand> {
        self.step()?;
        if !matches!(count, 1 | 2) || result >= count {
            return None;
        }
        let root = self.common_under(input.ty)?;
        let Ty::Chan {
            direction: Direction::Both | Direction::Receive,
            element,
        } = self.nodes[usize::from(root.0)]
        else {
            return None;
        };
        Some(Operand {
            mode: Mode::Value,
            ty: if result == 0 {
                element
            } else {
                self.basic(BasicKind::Bool)?
            },
            value: None,
        })
    }

    pub(crate) fn range_slot(
        &mut self,
        input: Operand,
        result: usize,
        count: usize,
        int_bits: u32,
    ) -> Option<TypeId> {
        self.step()?;
        if count == 0 || count > 2 || result >= count {
            return None;
        }
        let mut root = self.common_under(input.ty)?;
        let pointer = if let Ty::Pointer(element) = self.nodes[usize::from(root.0)] {
            root = self.underlying(element)?;
            true
        } else {
            false
        };
        match self.nodes[usize::from(root.0)].clone() {
            Ty::Array { element, .. } => {
                if result == 0 {
                    self.basic(BasicKind::Int)
                } else {
                    Some(element)
                }
            }
            Ty::Slice(element) if !pointer => {
                if result == 0 {
                    self.basic(BasicKind::Int)
                } else {
                    Some(element)
                }
            }
            Ty::Map { key, value } if !pointer => {
                self.comparable(key, 0)?.then_some(())?;
                Some(if result == 0 { key } else { value })
            }
            Ty::Chan {
                direction: Direction::Both | Direction::Receive,
                element,
            } if !pointer && count == 1 => Some(element),
            Ty::Basic(kind) if !pointer && kind.string() => self.basic(if result == 0 {
                BasicKind::Int
            } else {
                BasicKind::Int32
            }),
            Ty::Basic(kind) if !pointer && kind.integer() && count == 1 => {
                self.default_operand(input, int_bits)
            }
            Ty::Signature {
                parameters,
                results,
                variadic: false,
            } if !pointer && parameters.len() == 1 && results.is_empty() => {
                let callback = self.arguments[parameters.start];
                let callback = self.common_under(callback)?;
                let Ty::Signature {
                    parameters,
                    results,
                    variadic: false,
                } = self.nodes[usize::from(callback.0)].clone()
                else {
                    return None;
                };
                if parameters.len() > 2 || count > parameters.len() || results.len() != 1 {
                    return None;
                }
                let returned = self.arguments[results.start];
                let boolean = self.basic(BasicKind::Bool)?;
                self.identical(returned, boolean)?.then_some(())?;
                Some(self.arguments[parameters.start + result])
            }
            _ => None,
        }
    }
}
