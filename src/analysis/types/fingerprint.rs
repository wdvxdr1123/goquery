//! Bounded, request-local matching of scoped method-signature fingerprints.
use super::lookup::ScopedOwnerEnv;
use super::{Direction, MAX_BYTES, MAX_DEPTH, Ty, TypeArena, TypeId};
use crate::analysis::scope as locals;
use crate::index::IndexedSymbol as Symbol;
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

fn pack(tag: &str, parts: &[String]) -> Option<String> {
    let mut result = format!("{tag};");
    for part in parts {
        if result.len().saturating_add(part.len()).saturating_add(24) > MAX_BYTES {
            return None;
        }
        result.push_str(&format!("{}:{part}", part.len()));
    }
    Some(result)
}

type Parameter = (u8, usize);

enum Term<'a> {
    Atom(&'a str),
    Parameter(Parameter),
    Constructor(&'a str, Range<usize>),
}

struct Matcher<'a> {
    terms: Vec<Term<'a>>,
    edges: Vec<usize>,
    bindings: HashMap<Parameter, usize>,
    work: usize,
}

impl<'a> Matcher<'a> {
    fn parse(&mut self, text: &'a str, side: u8, depth: usize) -> Option<usize> {
        if depth > 64 || text.len() > MAX_BYTES || self.terms.len() >= 4096 {
            return None;
        }
        let (tag, mut tail) = text.split_once(';')?;
        let mut parts = Vec::new();
        while !tail.is_empty() {
            let (length, rest) = tail.split_once(':')?;
            let length: usize = length.parse().ok()?;
            parts.push(rest.get(..length)?);
            tail = rest.get(length..)?;
            if parts.len() > 4096 {
                return None;
            }
        }
        let term = match tag {
            "atom" if parts.len() == 1 => Term::Atom(parts[0]),
            "parameter" if parts.len() == 1 => Term::Parameter((side, parts[0].parse().ok()?)),
            "atom" | "parameter" => return None,
            _ => {
                let mut children = Vec::new();
                for part in parts {
                    children.push(self.parse(part, side, depth + 1)?);
                }
                let start = self.edges.len();
                self.edges.extend(children);
                Term::Constructor(tag, start..self.edges.len())
            }
        };
        if self.terms.len() >= 4096 {
            return None;
        }
        let id = self.terms.len();
        self.terms.push(term);
        Some(id)
    }

    fn step(&mut self) -> Option<()> {
        self.work = self.work.checked_sub(1)?;
        Some(())
    }

    fn resolve(&mut self, mut id: usize) -> Option<usize> {
        let mut path = Vec::new();
        while let Term::Parameter(parameter) = self.terms[id] {
            self.step()?;
            let Some(&next) = self.bindings.get(&parameter) else {
                break;
            };
            path.push(parameter);
            id = next;
        }
        for parameter in path {
            self.bindings.insert(parameter, id);
        }
        Some(id)
    }

    fn occurs(&mut self, parameter: Parameter, input: usize) -> Option<bool> {
        let mut pending = vec![input];
        let mut seen = vec![false; self.terms.len()];
        while let Some(id) = pending.pop() {
            self.step()?;
            let id = self.resolve(id)?;
            if seen[id] {
                continue;
            }
            seen[id] = true;
            match &self.terms[id] {
                Term::Parameter(other) if *other == parameter => return Some(true),
                Term::Constructor(_, children) => {
                    pending.extend_from_slice(&self.edges[children.clone()])
                }
                _ => {}
            }
        }
        Some(false)
    }

    fn unify(&mut self, left: usize, right: usize) -> Option<bool> {
        let mut pending = vec![(left, right)];
        while let Some((left, right)) = pending.pop() {
            self.step()?;
            let left = self.resolve(left)?;
            let right = self.resolve(right)?;
            if left == right {
                continue;
            }
            match (&self.terms[left], &self.terms[right]) {
                (Term::Parameter(a), Term::Parameter(b)) if a == b => {}
                (Term::Parameter(parameter), _) => {
                    let parameter = *parameter;
                    if self.occurs(parameter, right)? {
                        return Some(false);
                    }
                    self.bindings.insert(parameter, right);
                }
                (_, Term::Parameter(parameter)) => {
                    let parameter = *parameter;
                    if self.occurs(parameter, left)? {
                        return Some(false);
                    }
                    self.bindings.insert(parameter, left);
                }
                (Term::Atom(a), Term::Atom(b)) if a == b => {}
                (Term::Constructor(a, ac), Term::Constructor(b, bc))
                    if a == b && ac.len() == bc.len() =>
                {
                    pending.extend(
                        self.edges[ac.clone()]
                            .iter()
                            .copied()
                            .zip(self.edges[bc.clone()].iter().copied()),
                    );
                }
                _ => return Some(false),
            }
        }
        Some(true)
    }
}

/// Potential signature identity under some instantiation. Like gopls's method
/// fingerprints, this ignores constraints and starts fresh for each method.
pub(crate) fn fingerprints_match(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    if !left.contains("parameter;") && !right.contains("parameter;") {
        return false;
    }
    let mut matcher = Matcher {
        terms: Vec::new(),
        edges: Vec::new(),
        bindings: HashMap::new(),
        work: 65_536,
    };
    let Some((left, right)) = matcher.parse(left, 0, 0).zip(matcher.parse(right, 1, 0)) else {
        return false;
    };
    matcher.unify(left, right) == Some(true)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn implementation_method_fingerprint(
    workspace: &Workspace,
    set_file: &Path,
    set_owner: &Symbol,
    origin: &Path,
    owner_file: &Path,
    owner: &Symbol,
    is_receiver_method: bool,
    receiver_param_names: &[String],
    func_header: &str,
) -> Option<String> {
    if func_header.len() > MAX_BYTES {
        return None;
    }
    let signature = crate::syntax::parse_literal_signature(func_header);
    if !signature.diagnostics.is_empty() || !signature.type_parameters.is_empty() {
        return None;
    }
    let set_file = workspace.files.get_key_value(set_file)?.0;
    let origin = workspace.files.get_key_value(origin)?.0;
    let mut arena = TypeArena::new_symbolic(workspace);
    let owner_env_owned = if !owner.generic {
        if is_receiver_method && !receiver_param_names.is_empty() {
            return None;
        }
        Vec::new()
    } else {
        let set_param_names = if !set_owner.generic {
            Vec::new()
        } else {
            let end = set_owner
                .declaration()?
                .end
                .min(set_owner.end.saturating_add(MAX_BYTES));
            workspace
                .source_slice(set_file, set_owner.end..end)
                .and_then(|slice| locals::type_parameters(&slice))
                .or_else(|| {
                    let source = workspace.source(set_file)?;
                    locals::type_parameters(source.get(set_owner.end..)?)
                })?
        };
        let mut root_env_owned = Vec::with_capacity(set_param_names.len());
        for name in &set_param_names {
            let ty = arena.push_symbolic_parameter(set_file, name)?;
            root_env_owned.push((name.as_str(), ty));
        }
        ScopedOwnerEnv::owner_type_arguments(
            workspace,
            set_file,
            set_owner,
            &set_param_names,
            owner_file,
            owner,
            is_receiver_method,
            receiver_param_names,
            &mut arena,
            &root_env_owned,
        )?
    };
    let owner_env: Vec<_> = owner_env_owned
        .iter()
        .map(|(name, ty)| (name.as_str(), *ty))
        .collect();
    let sig_ty = arena.signature(origin, func_header, &signature, &owner_env, 0)?;
    arena.signature_fingerprint(sig_ty, 0)
}

impl TypeArena<'_> {
    fn package_key(&self, file: &Path) -> Option<String> {
        Some(format!(
            "{}:{}",
            file.parent()?.to_str()?,
            self.workspace.files.get(file)?.package
        ))
    }

    pub(super) fn signature_fingerprint(&mut self, sig_ty: TypeId, depth: usize) -> Option<String> {
        if depth >= MAX_DEPTH {
            return None;
        }
        self.step()?;
        let Ty::Signature {
            parameters,
            results,
            variadic,
        } = self.nodes.get(usize::from(sig_ty.0))?.clone()
        else {
            return None;
        };
        let param_ids = self.arguments[parameters].to_vec();
        let param_count = param_ids.len();
        let result_ids = self.arguments[results].to_vec();
        let mut encoded_params = Vec::with_capacity(param_count);
        let mut encoded_results = Vec::with_capacity(result_ids.len());
        let mut bytes = 0usize;
        for (index, param_ty) in param_ids.into_iter().enumerate() {
            let is_var = variadic && index + 1 == param_count;
            let inner = if is_var {
                let Ty::Slice(element) = self.nodes.get(usize::from(param_ty.0))? else {
                    return None;
                };
                *element
            } else {
                param_ty
            };
            let fp = self.type_fingerprint(inner, depth + 1)?;
            bytes = bytes.checked_add(fp.len() + 24)?;
            if bytes > MAX_BYTES {
                return None;
            }
            encoded_params.push(pack(if is_var { "variadic" } else { "type" }, &[fp])?);
        }
        for result_ty in result_ids {
            let fp = self.type_fingerprint(result_ty, depth + 1)?;
            bytes = bytes.checked_add(fp.len() + 24)?;
            if bytes > MAX_BYTES {
                return None;
            }
            encoded_results.push(pack("type", &[fp])?);
        }
        pack(
            "func",
            &[pack("in", &encoded_params)?, pack("out", &encoded_results)?],
        )
    }

    fn type_fingerprint(&mut self, input: TypeId, depth: usize) -> Option<String> {
        if depth >= MAX_DEPTH {
            return None;
        }
        self.step()?;
        let unaliased = self.unalias(input)?;
        match self.nodes.get(usize::from(unaliased.0))?.clone() {
            Ty::Parameter(index) => pack("parameter", &[index.to_string()]),
            Ty::Basic(super::BasicKind::UnsafePointer) => {
                pack("atom", &["unsafe.Pointer".to_owned()])
            }
            Ty::Basic(kind) => pack("atom", &[kind.canonical().name().to_owned()]),
            Ty::Error(_) => pack("atom", &["error".to_owned()]),
            Ty::Pointer(element) => pack("ptr", &[self.type_fingerprint(element, depth + 1)?]),
            Ty::Slice(element) => pack("slice", &[self.type_fingerprint(element, depth + 1)?]),
            Ty::Array { length, element } => pack(
                "array",
                &[
                    pack("atom", &[length.to_string()])?,
                    self.type_fingerprint(element, depth + 1)?,
                ],
            ),
            Ty::Map { key, value } => pack(
                "map",
                &[
                    self.type_fingerprint(key, depth + 1)?,
                    self.type_fingerprint(value, depth + 1)?,
                ],
            ),
            Ty::Chan { direction, element } => {
                let tag = match direction {
                    Direction::Both => "both",
                    Direction::Send => "send",
                    Direction::Receive => "recv",
                };
                pack(tag, &[self.type_fingerprint(element, depth + 1)?])
            }
            Ty::Signature { .. } => self.signature_fingerprint(unaliased, depth + 1),
            Ty::Interface { methods, .. } => {
                let methods = self.methods[methods].to_vec();
                if methods.is_empty() {
                    return pack("atom", &["any".to_owned()]);
                }
                let mut encoded = Vec::with_capacity(methods.len());
                for method in methods {
                    let package = if crate::syntax::is_exported_name(&method.name) {
                        String::new()
                    } else {
                        self.package_key(method.origin)?
                    };
                    let sig = self.signature_fingerprint(method.signature, depth + 1)?;
                    encoded.push(pack(
                        "method",
                        &[
                            pack("atom", &[method.name])?,
                            pack("atom", &[package])?,
                            sig,
                        ],
                    )?);
                }
                pack("interface", &encoded)
            }
            Ty::Struct { origin, fields } => {
                let fields = self.fields[fields].to_vec();
                let mut identities = Vec::with_capacity(fields.len());
                let mut bytes = 0usize;
                for field in fields {
                    let package = if crate::syntax::is_exported_name(&field.name) {
                        String::new()
                    } else {
                        self.package_key(origin)?
                    };
                    let tag: String = field.tag.iter().map(|b| format!("{b:02x}")).collect();
                    let identity = pack(
                        "field",
                        &[
                            pack("atom", &[field.name])?,
                            pack("atom", &[package])?,
                            pack("atom", &[field.embedded.to_string()])?,
                            pack("atom", &[tag])?,
                            self.type_fingerprint(field.ty, depth + 1)?,
                        ],
                    )?;
                    bytes = bytes.checked_add(identity.len() + 24)?;
                    if bytes > MAX_BYTES {
                        return None;
                    }
                    identities.push(identity);
                }
                pack("struct", &identities)
            }
            Ty::Named {
                declaration,
                arguments,
            } => {
                self.check_named_arguments(unaliased)?;
                let arg_ids = self.arguments[arguments.range()].to_vec();
                let mut parts = vec![pack(
                    "atom",
                    &[format!(
                        "{}:{}",
                        declaration.file.to_str()?,
                        declaration.symbol.start
                    )],
                )?];
                let mut bytes = parts[0].len();
                for argument in arg_ids {
                    let fp = self.type_fingerprint(argument, depth + 1)?;
                    bytes = bytes.checked_add(fp.len() + 24)?;
                    if bytes > MAX_BYTES {
                        return None;
                    }
                    parts.push(fp);
                }
                pack("named", &parts)
            }
            Ty::Alias { .. } | Ty::Tuple(_) | Ty::LocalDeclaration { .. } => None,
        }
    }
}
