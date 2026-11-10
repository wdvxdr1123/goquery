//! Breadth-first struct promotion with complete embedded interface method sets.
mod index;
mod interfaces;
use crate::analysis::member::MemberDeclaration;
use crate::analysis::resolve;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::MemberKind;
use crate::workspace::Workspace;
pub(crate) use index::MethodIndex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub(crate) enum ReceiverMember<'a> {
    Method(&'a PathBuf, &'a Symbol),
    Declaration(MemberDeclaration<'a>),
}

#[derive(Clone, Copy)]
enum Visibility {
    Selectors,
    Implementation,
}

struct Selection<'index, 'workspace> {
    scope: Option<&'index Path>,
    visibility: Visibility,
    complete_after: Option<&'index str>,
    methods: Option<&'index MethodIndex<'workspace>>,
    root: Option<(&'workspace PathBuf, &'workspace Symbol)>,
}

/// Private method/field names have package identity, even when embedded.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct MemberId<'a> {
    pub(crate) name: &'a str,
    package: Option<(&'a Path, &'a str)>,
}

impl Workspace {
    pub(crate) fn member_id<'a>(&'a self, file: &'a Path, name: &'a str) -> MemberId<'a> {
        MemberId {
            name,
            package: (!resolve::exported(name)).then(|| {
                (
                    file.parent().expect("source directory"),
                    self.files[file].package.as_str(),
                )
            }),
        }
    }

    pub(crate) fn implementation_members<'a>(
        &'a self,
        file: &Path,
        name: &str,
        required: Option<&[&str]>,
        methods: Option<&MethodIndex<'a>>,
        root: Option<(&'a PathBuf, &'a Symbol)>,
    ) -> Vec<ReceiverMember<'a>> {
        self.receiver_selection(
            file,
            name,
            Selection {
                scope: None,
                visibility: Visibility::Implementation,
                complete_after: required
                    .filter(|names| names.len() == 1 && resolve::exported(names[0]))
                    .map(|names| names[0]),
                methods,
                root,
            },
            |name| required.is_none_or(|names| names.contains(&name)),
            |member| match member {
                ReceiverMember::Method(..) => Some(member),
                ReceiverMember::Declaration(declaration)
                    if declaration.kind == MemberKind::Method =>
                {
                    Some(member)
                }
                _ => None,
            },
        )
    }

    pub(crate) fn receiver_methods(&self, file: &Path, name: &str) -> Vec<(&PathBuf, &Symbol)> {
        self.receiver_selection(
            file,
            name,
            Selection {
                scope: None,
                visibility: Visibility::Selectors,
                complete_after: None,
                methods: None,
                root: None,
            },
            |_| true,
            |member| match member {
                ReceiverMember::Method(path, symbol) => Some((path, symbol)),
                ReceiverMember::Declaration(_) => None,
            },
        )
    }

    pub(crate) fn receiver_members(&self, file: &Path, name: &str) -> Vec<ReceiverMember<'_>> {
        self.scoped_receiver_members(file, file, name)
    }

    pub(crate) fn scoped_receiver_members(
        &self,
        file: &Path,
        scope: &Path,
        name: &str,
    ) -> Vec<ReceiverMember<'_>> {
        self.receiver_selection(
            file,
            name,
            Selection {
                scope: Some(scope),
                visibility: Visibility::Selectors,
                complete_after: None,
                methods: None,
                root: None,
            },
            |_| true,
            Some,
        )
    }

    pub(crate) fn receiver_member(
        &self,
        file: &Path,
        receiver: &str,
        name: &str,
    ) -> Option<ReceiverMember<'_>> {
        self.scoped_receiver_member(file, file, receiver, name)
    }

    pub(crate) fn scoped_receiver_member(
        &self,
        file: &Path,
        scope: &Path,
        receiver: &str,
        name: &str,
    ) -> Option<ReceiverMember<'_>> {
        self.receiver_selection(
            file,
            receiver,
            Selection {
                scope: Some(scope),
                visibility: Visibility::Selectors,
                complete_after: Some(name),
                methods: None,
                root: None,
            },
            |candidate| candidate == name,
            Some,
        )
        .pop()
    }

    fn receiver_selection<'a, T>(
        &'a self,
        file: &Path,
        name: &str,
        selection: Selection<'_, 'a>,
        matches: impl Fn(&str) -> bool,
        select: impl Fn(ReceiverMember<'a>) -> Option<T>,
    ) -> Vec<T> {
        // Selector compatibility still uses named receiver roots. Preserve the
        // value's pointer type in inference and normalize only at this boundary.
        // Full pointer/value method-set filtering is a separate semantic step.
        let normalized;
        let name = if name.trim_start().starts_with('*') {
            normalized = crate::analysis::scope::named_type(name);
            let Some(name) = normalized.as_deref() else {
                return Vec::new();
            };
            name
        } else {
            name
        };
        let scope = selection.scope.unwrap_or(file);
        let Some((root, ty)) = selection.root.or_else(|| self.embedded_target(scope, name)) else {
            return Vec::new();
        };
        let mut level = vec![(root, ty, scope, name, Vec::new())];
        let mut chosen = BTreeMap::new();
        let mut visited = 0;
        for _ in 0..128 {
            if level.is_empty() {
                break;
            }
            let mut names: BTreeMap<MemberId<'_>, Option<T>> = BTreeMap::new();
            let mut next = Vec::new();
            let mut uncertain_next = false;
            for (path, ty, scope, spelling, mut ancestors) in level {
                let identity = (path, ty.start);
                if ancestors.contains(&identity) {
                    continue;
                }
                visited += 1;
                if visited > 4096 {
                    return Vec::new();
                }
                ancestors.push(identity);
                let same = matches!(selection.visibility, Visibility::Implementation)
                    || path.parent() == file.parent()
                        && self.files[path].package == self.files[file].package;
                let shape = if ty
                    .type_info
                    .as_ref()
                    .is_some_and(|info| info.target.is_some())
                {
                    self.underlying_target(scope, spelling)
                } else {
                    Some((path, ty))
                };
                let fallback;
                let methods = if let Some(index) = selection.methods {
                    index.get(self, path, &ty.name)
                } else {
                    fallback = self.symbols(path);
                    &fallback
                };
                for &(method_path, method) in methods {
                    if matches(&method.name)
                        && method.receiver.as_deref() == Some(ty.name.as_str())
                        && (matches!(selection.visibility, Visibility::Implementation)
                            || self.selector_file_visible(file, method_path))
                        && (same || resolve::exported(&method.name))
                        && !shape.is_some_and(|(_, shape)| {
                            shape
                                .type_info
                                .as_ref()
                                .is_some_and(|info| info.is_interface())
                        })
                    {
                        names
                            .entry(self.member_id(method_path, &method.name))
                            .and_modify(|slot| *slot = None)
                            .or_insert_with(|| select(ReceiverMember::Method(method_path, method)));
                    }
                }
                if let Some((field_path, owner)) = shape
                    && let Some(info) = &owner.type_info
                {
                    if info.is_interface() {
                        let consumer =
                            matches!(selection.visibility, Visibility::Selectors).then_some(file);
                        let Some(methods) = self.interface_members(consumer, field_path, owner)
                        else {
                            return Vec::new();
                        };
                        for member in methods {
                            if matches(member.name) {
                                names
                                    .entry(self.member_id(member.file, member.name))
                                    .and_modify(|slot| *slot = None)
                                    .or_insert_with(|| select(ReceiverMember::Declaration(member)));
                            }
                        }
                        continue;
                    }
                    let same = matches!(selection.visibility, Visibility::Implementation)
                        || field_path.parent() == file.parent()
                            && self.files[field_path].package == self.files[file].package;
                    for member in &info.members {
                        if matches(member) && (same || resolve::exported(member)) {
                            let declaration = MemberDeclaration {
                                file: field_path,
                                owner,
                                name: member,
                                kind: if info.is_interface() {
                                    MemberKind::Method
                                } else {
                                    MemberKind::Field
                                },
                                embedded: false,
                            };
                            names
                                .entry(self.member_id(field_path, member))
                                .and_modify(|slot| *slot = None)
                                .or_insert_with(|| {
                                    select(ReceiverMember::Declaration(declaration))
                                });
                        }
                    }
                    for embedded in &info.embedded {
                        let field = crate::analysis::scope::named_root(embedded)
                            .rsplit('.')
                            .next()
                            .expect("embedded name");
                        if matches(field)
                            && (same || resolve::exported(field))
                            && !info.is_interface()
                        {
                            let declaration = MemberDeclaration {
                                file: field_path,
                                owner,
                                name: field,
                                kind: MemberKind::Field,
                                embedded: true,
                            };
                            names
                                .entry(self.member_id(field_path, field))
                                .and_modify(|slot| *slot = None)
                                .or_insert_with(|| {
                                    select(ReceiverMember::Declaration(declaration))
                                });
                        }
                        if let Some((base_path, base)) = self.embedded_target(field_path, embedded)
                        {
                            next.push((
                                base_path,
                                base,
                                field_path.as_path(),
                                embedded.as_str(),
                                ancestors.clone(),
                            ));
                            if next.len() > 4096 {
                                return Vec::new();
                            }
                        } else if !matches!(
                            embedded.as_str(),
                            "bool"
                                | "byte"
                                | "rune"
                                | "string"
                                | "int"
                                | "int8"
                                | "int16"
                                | "int32"
                                | "int64"
                                | "uint"
                                | "uint8"
                                | "uint16"
                                | "uint32"
                                | "uint64"
                                | "uintptr"
                                | "float32"
                                | "float64"
                                | "complex64"
                                | "complex128"
                        ) {
                            uncertain_next = true;
                        }
                    }
                }
            }
            for (name, method) in names {
                chosen.entry(name).or_insert(method);
            }
            if selection
                .complete_after
                .is_some_and(|name| chosen.keys().any(|key| key.name == name))
            {
                return chosen.into_values().flatten().collect();
            }
            if uncertain_next {
                return chosen.into_values().flatten().collect();
            }
            level = next;
        }
        if !level.is_empty() {
            return Vec::new();
        }
        chosen.into_values().flatten().collect()
    }
}
