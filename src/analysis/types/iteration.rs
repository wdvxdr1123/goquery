//! Range declarations derive iteration types without a consumer text round trip.
use super::{BasicKind, Direction, Mode, Operand, Ty, TypeArena, TypeId};

impl TypeArena<'_> {
    pub(crate) fn send_element(&mut self, input: TypeId) -> Option<TypeId> {
        self.step()?;
        let root = self.common_under(input)?;
        match self.node(root)? {
            Ty::Chan {
                direction: Direction::Both | Direction::Send,
                element,
            } => Some(*element),
            _ => None,
        }
    }

    pub(crate) fn receive(&mut self, input: Operand) -> Option<Operand> {
        self.step()?;
        let root = self.common_under(input.ty)?;
        let Ty::Chan {
            direction: Direction::Both | Direction::Receive,
            element,
        } = *self.node(root)?
        else {
            return None;
        };
        Some(Operand {
            mode: Mode::CommaOk,
            ty: element,
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
        let pointer = if let Ty::Pointer(element) = *self.node(root)? {
            root = self.underlying(element)?;
            true
        } else {
            false
        };
        match *self.node(root)? {
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
            Ty::Signature(_) if !pointer => {
                let sig = self.signature_data(root)?.clone();
                if sig.variadic || sig.parameters.len() != 1 || !sig.results.is_empty() {
                    return None;
                }
                let callback = self.arg_range(root, sig.parameters)[0];
                let callback = self.common_under(callback)?;
                let cb_sig = self.signature_data(callback)?.clone();
                if cb_sig.variadic
                    || cb_sig.parameters.len() > 2
                    || count > cb_sig.parameters.len()
                    || cb_sig.results.len() != 1
                {
                    return None;
                }
                let returned = self.arg_range(callback, cb_sig.results)[0];
                let boolean = self.basic(BasicKind::Bool)?;
                self.identical(returned, boolean)?.then_some(())?;
                Some(self.arg_range(callback, cb_sig.parameters)[result])
            }
            _ => None,
        }
    }
}
