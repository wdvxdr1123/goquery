//! Type relation predicates (`identical`, `assignable`, `convertible`, `comparable`,
//! `implements`) extracted into [`TypeRelations`].
use super::constant::{constant_value, representable};
use super::eval::TypeEval;
use super::ty::CheckMode;
use super::{
    BasicKind, CheckError, ConstAssignError, Constant, Direction, MAX_DEPTH, MAX_NODES, Mode,
    Operand, Tags, Ty, TypeId, TypeStore,
};
use CheckError::{Invalid, Unknown};

#[derive(Clone, Copy)]
pub(crate) enum AssignmentError {
    TypeMismatch,
    InterfaceMismatch,
    ImplicitConstant(ConstAssignError),
}

#[derive(Clone, Copy)]
pub(crate) enum ConversionError {
    Overflow(i128),
    InvalidConstant,
}

pub(super) enum TypePairs {
    Mismatch,
    Children(Vec<(TypeId, TypeId)>),
}

/// Standalone Go type-predicate evaluator over [`TypeEval`].
pub(crate) struct TypeRelations<'s, 'a> {
    eval: TypeEval<'s, 'a>,
}

impl<'s, 'a> TypeRelations<'s, 'a> {
    pub(crate) fn new(store: &'s mut TypeStore<'a>, mode: CheckMode) -> Self {
        Self {
            eval: TypeEval::new(store, mode),
        }
    }

    #[allow(dead_code)]
    pub(crate) const fn mode(&self) -> CheckMode {
        self.eval.mode()
    }

    pub(super) fn store(&self) -> &TypeStore<'a> {
        self.eval.store()
    }

    fn store_mut(&mut self) -> &mut TypeStore<'a> {
        self.eval.store_mut()
    }

    pub(crate) fn identical(&mut self, left: TypeId, right: TypeId) -> Option<bool> {
        self.identical_with_tags(left, right, Tags::Include)
    }

    pub(super) fn identical_with_tags(
        &mut self,
        left: TypeId,
        right: TypeId,
        tags: Tags,
    ) -> Option<bool> {
        let mut pending = vec![(left, right, 0usize)];
        while let Some((left, right, depth)) = pending.pop() {
            self.eval.step()?;
            if depth >= MAX_DEPTH {
                return None;
            }
            let (left, right) = (
                self.store_mut().unalias(left)?,
                self.store_mut().unalias(right)?,
            );
            let TypePairs::Children(pairs) = self.identity_pairs(left, right, tags)? else {
                return Some(false);
            };
            if pending.len() + pairs.len() > MAX_NODES {
                return None;
            }
            pending.extend(pairs.into_iter().map(|(a, b)| (a, b, depth + 1)));
        }
        Some(true)
    }

    /// Decompose one pair of unaliased types without deciding how formal
    /// parameters bind. Identity and implementation schema matching share
    /// constructor, declaration, package, field/tag and variadic rules.
    pub(super) fn identity_pairs(
        &mut self,
        left: TypeId,
        right: TypeId,
        tags: Tags,
    ) -> Option<TypePairs> {
        if let (Some((_, la, lb, _)), Some((_, ra, rb, _))) = (
            self.store().local_decl_info(left),
            self.store().local_decl_info(right),
        ) {
            return Some(if la == ra && lb == rb {
                TypePairs::Children(Vec::new())
            } else if self.store().is_known_non_opaque(left)
                && self.store().is_known_non_opaque(right)
            {
                TypePairs::Mismatch
            } else {
                return None;
            });
        }
        if self.store().local_origin_id(left).is_some()
            || self.store().local_origin_id(right).is_some()
        {
            return if self.store().is_known_non_opaque(left)
                && self.store().is_known_non_opaque(right)
            {
                Some(TypePairs::Mismatch)
            } else {
                None
            };
        }
        if self.store().error_interface(left).is_some()
            && self.store().error_interface(right).is_some()
        {
            return Some(TypePairs::Children(Vec::new()));
        }
        if self.store().error_interface(left).is_some()
            || self.store().error_interface(right).is_some()
        {
            return Some(TypePairs::Mismatch);
        }
        let pairs = match (*self.store().node(left)?, *self.store().node(right)?) {
            (Ty::Basic(a), Ty::Basic(b)) => {
                if a.canonical() != b.canonical() {
                    return Some(TypePairs::Mismatch);
                }
                Vec::new()
            }
            (Ty::TypeParam(_), Ty::TypeParam(_)) => {
                if left != right {
                    return Some(TypePairs::Mismatch);
                }
                Vec::new()
            }
            (Ty::Named { .. }, Ty::Named { .. }) => {
                let (a, aa, _) = self.store().named_info(left)?;
                let (b, ba, _) = self.store().named_info(right)?;
                if a.file != b.file || a.symbol.start != b.symbol.start || aa.len() != ba.len() {
                    return Some(TypePairs::Mismatch);
                }
                // A generic declaration used as a value type must be instantiated.
                if a.symbol.generic && aa.is_empty() {
                    return None;
                }
                self.store()
                    .arguments_slice(left, aa)
                    .iter()
                    .copied()
                    .zip(self.store().arguments_slice(right, ba).iter().copied())
                    .collect()
            }
            (Ty::Tuple(a), Ty::Tuple(b)) => {
                if a.len() != b.len() {
                    return Some(TypePairs::Mismatch);
                }
                self.store()
                    .arguments_slice(left, a)
                    .iter()
                    .copied()
                    .zip(self.store().arguments_slice(right, b).iter().copied())
                    .collect()
            }
            (Ty::Pointer(a), Ty::Pointer(b)) | (Ty::Slice(a), Ty::Slice(b)) => vec![(a, b)],
            (Ty::Interface(_), Ty::Interface(_)) => {
                let ld = self.store().interface_data(left)?.clone();
                let rd = self.store().interface_data(right)?.clone();
                if ld.comparable != rd.comparable
                    || ld.has_type_elements != rd.has_type_elements
                    || ld.embeddeds.len() != rd.embeddeds.len()
                    || !self.store().same_interface_methods(
                        left,
                        ld.methods.clone(),
                        right,
                        rd.methods.clone(),
                    )?
                {
                    return Some(TypePairs::Mismatch);
                }
                let mut pairs: Vec<(TypeId, TypeId)> = self
                    .store()
                    .methods_slice(left, ld.methods)
                    .iter()
                    .map(|m| m.signature)
                    .zip(
                        self.store()
                            .methods_slice(right, rd.methods)
                            .iter()
                            .map(|m| m.signature),
                    )
                    .collect();
                pairs.extend(
                    self.store()
                        .arguments_slice(left, ld.embeddeds)
                        .iter()
                        .copied()
                        .zip(
                            self.store()
                                .arguments_slice(right, rd.embeddeds)
                                .iter()
                                .copied(),
                        ),
                );
                pairs
            }
            (Ty::Union(a), Ty::Union(b)) => {
                let at = self.store().terms_slice(left, a);
                let bt = self.store().terms_slice(right, b);
                if at.len() != bt.len()
                    || at
                        .iter()
                        .zip(bt)
                        .any(|(x, y)| x.approximate != y.approximate)
                {
                    return Some(TypePairs::Mismatch);
                }
                at.iter()
                    .map(|x| x.ty)
                    .zip(bt.iter().map(|y| y.ty))
                    .collect()
            }
            (Ty::Signature(_), Ty::Signature(_)) => {
                let ls = self.store().signature_data(left)?.clone();
                let rs = self.store().signature_data(right)?.clone();
                if ls.variadic != rs.variadic
                    || ls.parameters.len() != rs.parameters.len()
                    || ls.results.len() != rs.results.len()
                {
                    return Some(TypePairs::Mismatch);
                }
                self.store()
                    .arg_range(left, ls.parameters)
                    .iter()
                    .chain(self.store().arg_range(left, ls.results))
                    .copied()
                    .zip(
                        self.store()
                            .arg_range(right, rs.parameters)
                            .iter()
                            .chain(self.store().arg_range(right, rs.results))
                            .copied(),
                    )
                    .collect()
            }
            (Ty::Struct(_), Ty::Struct(_)) => {
                let ld = self.store().struct_data(left)?.clone();
                let rd = self.store().struct_data(right)?.clone();
                if !self.store().struct_fields_with_tags(
                    (ld.origin, left, ld.fields.clone()),
                    (rd.origin, right, rd.fields.clone()),
                    tags,
                )? {
                    return Some(TypePairs::Mismatch);
                }
                self.store()
                    .fields_slice(left, ld.fields)?
                    .iter()
                    .map(|f| f.ty)
                    .zip(
                        self.store()
                            .fields_slice(right, rd.fields)?
                            .iter()
                            .map(|f| f.ty),
                    )
                    .collect()
            }
            (
                Ty::Array {
                    length: a,
                    element: ae,
                },
                Ty::Array {
                    length: b,
                    element: be,
                },
            ) if a == b => vec![(ae, be)],
            (Ty::Map { key: a, value: av }, Ty::Map { key: b, value: bv }) => {
                vec![(a, b), (av, bv)]
            }
            (
                Ty::Chan {
                    direction: a,
                    element: ae,
                },
                Ty::Chan {
                    direction: b,
                    element: be,
                },
            ) if a == b => vec![(ae, be)],
            _ => return Some(TypePairs::Mismatch),
        };
        Some(TypePairs::Children(pairs))
    }

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
        self.eval.step().ok_or(Unknown)?;
        let mismatch = Invalid(AssignmentError::TypeMismatch);
        if !source.mode.is_value() {
            return Err(mismatch);
        }
        // Compare declaration identities before expanding types. Interface
        // constructors still need their complete basic method set.
        if !self
            .store_mut()
            .interface_constructor(target)
            .ok_or(Unknown)?
            && self.identical(source.ty, target).ok_or(Unknown)?
        {
            return Ok(());
        }
        if self.store_mut().is_interface(target).ok_or(Unknown)? {
            return self
                .store_mut()
                .assign_interface(source, target, int_bits)
                .ok_or(Unknown)?
                .then_some(())
                .ok_or(Invalid(AssignmentError::InterfaceMismatch));
        }
        if let Some(kind) = self
            .store_mut()
            .basic_kind(source.ty)
            .filter(|kind| kind.untyped())
        {
            if kind == BasicKind::UntypedNil {
                return self
                    .nilable(target)
                    .ok_or(Unknown)?
                    .then_some(())
                    .ok_or(mismatch);
            }
            if self
                .store_mut()
                .assign_basic(source, target, int_bits)
                .is_some()
            {
                return Ok(());
            }
            let reason = self
                .store_mut()
                .const_assignment_error(source, target, int_bits)
                .ok_or(Unknown)?;
            return Err(Invalid(AssignmentError::ImplicitConstant(reason)));
        }
        if self.identical(source.ty, target).ok_or(Unknown)? {
            return Ok(());
        }
        let (source, target) = (
            self.store_mut().unalias(source.ty).ok_or(Unknown)?,
            self.store_mut().unalias(target).ok_or(Unknown)?,
        );
        if self.store().is_defined_type(source) && self.store().is_defined_type(target) {
            return Err(mismatch);
        }
        let (source, target) = (
            self.store_mut().underlying(source).ok_or(Unknown)?,
            self.store_mut().underlying(target).ok_or(Unknown)?,
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
            self.store().node(source).ok_or(Unknown)?,
            self.store().node(target).ok_or(Unknown)?,
        ) {
            return self
                .identical(*source, *target)
                .ok_or(Unknown)?
                .then_some(())
                .ok_or(mismatch);
        }
        Err(mismatch)
    }

    pub(crate) fn convert(
        &mut self,
        input: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<Operand> {
        self.checked_conversion(input, target, int_bits)?.ok()
    }

    pub(crate) fn checked_conversion(
        &mut self,
        input: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<Result<Operand, ConversionError>> {
        self.store_mut().underlying(target)?;
        self.store_mut().underlying(input.ty)?;
        let source_kind = self.store_mut().basic_kind(input.ty);
        if source_kind == Some(BasicKind::UntypedNil) {
            self.nilable(target)?.then_some(())?;
            return Some(Ok(Operand {
                mode: Mode::Value,
                ty: target,
                value: None,
            }));
        }
        if self.store_mut().is_interface(target)? {
            if !self.assignable(input, target, int_bits)? && !self.convertible(input.ty, target)? {
                return None;
            }
            return Some(Ok(Operand {
                mode: Mode::Value,
                ty: target,
                value: None,
            }));
        }
        let target_kind = self.store_mut().basic_kind(target);
        if input.mode == Mode::Constant
            && let Some(kind) = target_kind
        {
            let source = source_kind?;
            let value = if source.integer() && kind.string() {
                let Constant::Int(value) = input.value? else {
                    return None;
                };
                let rune = u32::try_from(value)
                    .ok()
                    .and_then(char::from_u32)
                    .unwrap_or(char::REPLACEMENT_CHARACTER);
                Constant::String {
                    length: Some(rune.len_utf8() as u64),
                }
            } else {
                let value = input.value?;
                if !representable(value, kind, int_bits) {
                    let error = if kind.integer() && source.integer() {
                        ConversionError::Overflow(value.integer_value()?)
                    } else if kind != BasicKind::UnsafePointer
                        && (kind.integer()
                            && (value.integer_value().is_some() || value.definitely_fractional())
                            || source.boolean() != kind.boolean()
                            || source.string() != kind.string())
                    {
                        ConversionError::InvalidConstant
                    } else {
                        return None;
                    };
                    return Some(Err(error));
                }
                constant_value(value, kind)?
            };
            return Some(Ok(Operand {
                mode: Mode::Constant,
                ty: target,
                value: Some(value),
            }));
        }
        self.convertible(input.ty, target)?.then_some(Ok(Operand {
            mode: Mode::Value,
            ty: target,
            value: None,
        }))
    }

    pub(super) fn nilable(&mut self, target: TypeId) -> Option<bool> {
        let target = self.store_mut().underlying(target)?;
        Some(matches!(
            self.store().node(target)?,
            Ty::Pointer(_)
                | Ty::Slice(_)
                | Ty::Map { .. }
                | Ty::Chan { .. }
                | Ty::Signature { .. }
                | Ty::Interface { .. }
                | Ty::Basic(BasicKind::UnsafePointer)
        ))
    }

    pub(crate) fn convertible(&mut self, source: TypeId, target: TypeId) -> Option<bool> {
        if self.identical(source, target)? {
            return Some(true);
        }
        let (s, t) = (
            self.store_mut().underlying(source)?,
            self.store_mut().underlying(target)?,
        );
        if self.identical_with_tags(s, t, Tags::Ignore)? {
            return Some(true);
        }
        let (sv, tv) = (
            self.store_mut().unalias(source)?,
            self.store_mut().unalias(target)?,
        );
        if let (Ty::Pointer(a), Ty::Pointer(b)) = (*self.store().node(sv)?, *self.store().node(tv)?)
        {
            let (a, b) = (
                self.store_mut().underlying(a)?,
                self.store_mut().underlying(b)?,
            );
            if self.identical_with_tags(a, b, Tags::Ignore)? {
                return Some(true);
            }
        }
        if let (Some(a), Some(b)) = (
            self.store_mut().basic_kind(s),
            self.store_mut().basic_kind(t),
        ) {
            if let (Some(a), Some(b)) = (a.numeric_rank(), b.numeric_rank()) {
                return Some(a <= 2 && b <= 2 || a == 3 && b == 3);
            }
            if a.integer() && b.string() {
                return Some(true);
            }
        }
        if (matches!(self.store().node(s)?, Ty::Pointer(_))
            || self.store_mut().basic_kind(s) == Some(BasicKind::Uintptr))
            && self.store_mut().basic_kind(t) == Some(BasicKind::UnsafePointer)
        {
            return Some(true);
        }
        if self.store_mut().basic_kind(s) == Some(BasicKind::UnsafePointer)
            && (matches!(self.store().node(t)?, Ty::Pointer(_))
                || self.store_mut().basic_kind(t) == Some(BasicKind::Uintptr))
        {
            return Some(true);
        }
        if self
            .store_mut()
            .basic_kind(s)
            .is_some_and(BasicKind::string)
            && self.bytes_or_runes(t)?
            || self.bytes_or_runes(s)?
                && self
                    .store_mut()
                    .basic_kind(t)
                    .is_some_and(BasicKind::string)
        {
            return Some(true);
        }
        match (*self.store().node(s)?, *self.store().node(t)?) {
            (
                Ty::Chan {
                    direction: Direction::Both,
                    element: a,
                },
                Ty::Chan { element: b, .. },
            ) if !self.store().is_defined_type(sv) || !self.store().is_defined_type(tv) => {
                self.identical(a, b)
            }
            (
                Ty::Slice(element),
                Ty::Array {
                    element: target, ..
                },
            ) => self.identical(element, target),
            (Ty::Slice(element), Ty::Pointer(array)) => {
                let array = self.store_mut().underlying(array)?;
                let Ty::Array {
                    element: target, ..
                } = *self.store().node(array)?
                else {
                    return Some(false);
                };
                self.identical(element, target)
            }
            _ => Some(false),
        }
    }

    fn bytes_or_runes(&mut self, input: TypeId) -> Option<bool> {
        let input = self.store_mut().underlying(input)?;
        let Ty::Slice(element) = *self.store().node(input)? else {
            return Some(false);
        };
        Some(
            self.store_mut().basic_kind(element).is_some_and(|kind| {
                matches!(kind.canonical(), BasicKind::Uint8 | BasicKind::Int32)
            }),
        )
    }
}

impl TypeStore<'_> {
    pub(crate) fn identical(&mut self, left: TypeId, right: TypeId) -> Option<bool> {
        TypeEval::from_store(self)
            .relations()
            .identical(left, right)
    }

    pub(super) fn identity_pairs(
        &mut self,
        left: TypeId,
        right: TypeId,
        tags: Tags,
    ) -> Option<TypePairs> {
        TypeEval::from_store(self)
            .relations()
            .identity_pairs(left, right, tags)
    }

    pub(crate) fn assignable(
        &mut self,
        source: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<bool> {
        TypeEval::from_store(self)
            .relations()
            .assignable(source, target, int_bits)
    }

    pub(crate) fn check_assignment(
        &mut self,
        source: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Result<(), CheckError<AssignmentError>> {
        TypeEval::from_store(self)
            .relations()
            .check_assignment(source, target, int_bits)
    }

    pub(crate) fn convert(
        &mut self,
        input: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<Operand> {
        TypeEval::from_store(self)
            .relations()
            .convert(input, target, int_bits)
    }

    pub(crate) fn checked_conversion(
        &mut self,
        input: Operand,
        target: TypeId,
        int_bits: u32,
    ) -> Option<Result<Operand, ConversionError>> {
        TypeEval::from_store(self)
            .relations()
            .checked_conversion(input, target, int_bits)
    }

    /// Test-only structural projection; no JSON machinery enters runtime queries.
    #[cfg(test)]
    pub(crate) fn test_identity(&mut self, input: TypeId) -> Option<serde_json::Value> {
        self.identity_projection(input, 0)
    }

    #[cfg(test)]
    fn identity_projection(&mut self, input: TypeId, depth: usize) -> Option<serde_json::Value> {
        use serde_json::json;
        self.step()?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let input = self.unalias(input)?;
        if self.local_decl_info(input).is_some() {
            return self.local_declaration_identity(input);
        }
        if self.error_interface(input).is_some() {
            return Some(json!(["universe", "error"]));
        }
        Some(match *self.node(input)? {
            Ty::Invalid | Ty::Union(_) => return None,
            Ty::TypeParam(_) => {
                let (origin, selection, _) = self.type_param_info(input)?;
                json!(["parameter", origin, selection])
            }
            Ty::Interface(_) => {
                let methods_range = self.interface_data(input)?.methods.clone();
                let methods = self.methods_slice(input, methods_range).to_vec();
                let mut values = Vec::new();
                for method in methods {
                    let package = if crate::syntax::is_exported_name(&method.name) {
                        String::new()
                    } else {
                        self.workspace.import_path(method.origin)?
                    };
                    values.push(json!([
                        method.name,
                        package,
                        self.identity_projection(method.signature, depth + 1)?
                    ]));
                }
                json!(["interface", values])
            }
            Ty::Signature(_) => {
                let sig = self.signature_data(input)?.clone();
                let (parameters, results, variadic) = (
                    self.arg_range(input, sig.parameters).to_vec(),
                    self.arg_range(input, sig.results).to_vec(),
                    sig.variadic,
                );
                let parameters: Vec<_> = parameters
                    .into_iter()
                    .map(|ty| self.identity_projection(ty, depth + 1))
                    .collect::<Option<_>>()?;
                let results: Vec<_> = results
                    .into_iter()
                    .map(|ty| self.identity_projection(ty, depth + 1))
                    .collect::<Option<_>>()?;
                json!(["signature", variadic, parameters, results])
            }
            Ty::Struct(_) => {
                let data = self.struct_data(input)?.clone();
                let origin = data.origin;
                let fields = self.fields_slice(input, data.fields)?.to_vec();
                let mut values = Vec::new();
                for field in fields {
                    let package = if crate::syntax::is_exported_name(&field.name) {
                        String::new()
                    } else {
                        self.workspace.import_path(origin)?
                    };
                    values.push(json!([
                        field.name,
                        package,
                        field.embedded,
                        field.tag,
                        self.identity_projection(field.ty, depth + 1)?
                    ]));
                }
                json!(["struct", values])
            }
            Ty::Basic(kind) => json!(["basic", kind.canonical().name()]),
            Ty::Named { .. } => {
                let (declaration, arguments, _) = self.named_info(input)?;
                let package = self.workspace.import_path(declaration.file)?;
                let name = declaration.symbol.name.clone();
                let arguments = self.arguments_slice(input, arguments).to_vec();
                let arguments: Vec<_> = arguments
                    .into_iter()
                    .map(|argument| self.identity_projection(argument, depth + 1))
                    .collect::<Option<_>>()?;
                json!(["named", package, name, arguments])
            }
            Ty::Alias { .. } => return None,
            Ty::Tuple(elements) => {
                let elements = self.arguments_slice(input, elements).to_vec();
                let elements: Vec<_> = elements
                    .into_iter()
                    .map(|element| self.identity_projection(element, depth + 1))
                    .collect::<Option<_>>()?;
                json!(["tuple", elements])
            }
            Ty::Pointer(element) | Ty::Slice(element) => {
                let tag = if matches!(self.node(input)?, Ty::Pointer(_)) {
                    "pointer"
                } else {
                    "slice"
                };
                json!([tag, self.identity_projection(element, depth + 1)?])
            }
            Ty::Array { length, element } => json!([
                "array",
                length,
                self.identity_projection(element, depth + 1)?
            ]),
            Ty::Map { key, value } => json!([
                "map",
                self.identity_projection(key, depth + 1)?,
                self.identity_projection(value, depth + 1)?
            ]),
            Ty::Chan { direction, element } => {
                let direction = match direction {
                    Direction::Both => "both",
                    Direction::Send => "send",
                    Direction::Receive => "receive",
                };
                json!([
                    "channel",
                    direction,
                    self.identity_projection(element, depth + 1)?
                ])
            }
        })
    }

    #[cfg(test)]
    fn local_declaration_identity(&mut self, input: TypeId) -> Option<serde_json::Value> {
        let (_, origin, selection, _) = self.local_decl_info(input)?;
        Some(serde_json::json!(["local", origin, selection.start, []]))
    }
}
