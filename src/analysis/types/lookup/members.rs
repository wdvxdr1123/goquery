//! Breadth-first struct promotion, receiver method sets, and interface unions.
use super::{embedded_target, underlying_target};
use crate::analysis::obj::{MemberDeclaration, ReceiverMember};
use crate::analysis::resolve;
use crate::index::IndexedSymbol as Symbol;
use crate::syntax::MemberKind;
use crate::workspace::Workspace;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

type Owner<'a> = (Option<&'a Path>, &'a str, &'a str);
type Method<'a> = (&'a PathBuf, &'a Symbol);

pub(crate) struct MethodIndex<'a> {
    methods: HashMap<Owner<'a>, Vec<Method<'a>>>,
}

impl<'a> MethodIndex<'a> {
    pub(crate) fn new(workspace: &'a Workspace) -> Option<Self> {
        let mut methods: HashMap<Owner<'_>, Vec<Method<'_>>> = HashMap::new();
        let mut count = 0;
        // Expansion may use declarations outside the query's candidate view.
        // Index the complete metadata graph, then narrow result candidates.
        for (file, stub) in &workspace.files {
            for symbol in &stub.symbols {
                if let Some(receiver) = &symbol.receiver {
                    count += 1;
                    if count > 65_536 {
                        return None;
                    }
                    methods
                        .entry((file.parent(), &stub.package, receiver))
                        .or_default()
                        .push((file, symbol));
                    if methods.len() > 16_384 {
                        return None;
                    }
                }
            }
        }
        Some(Self { methods })
    }

    pub(crate) fn get(
        &self,
        workspace: &'a Workspace,
        file: &'a Path,
        name: &'a str,
    ) -> &[Method<'a>] {
        self.methods
            .get(&(file.parent(), &workspace.files[file].package, name))
            .map_or(&[], Vec::as_slice)
    }
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

pub(crate) fn member_id<'a>(
    workspace: &'a Workspace,
    file: &'a Path,
    name: &'a str,
) -> MemberId<'a> {
    MemberId {
        name,
        package: (!resolve::exported(name)).then(|| {
            (
                file.parent().expect("source directory"),
                workspace.files[file].package.as_str(),
            )
        }),
    }
}

pub(crate) fn implementation_members<'a>(
    workspace: &'a Workspace,
    file: &Path,
    name: &str,
    required: Option<&[&str]>,
    methods: Option<&MethodIndex<'a>>,
    root: Option<(&'a PathBuf, &'a Symbol)>,
) -> Vec<ReceiverMember<'a>> {
    receiver_selection(
        workspace,
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
            ReceiverMember::Declaration(declaration) if declaration.kind == MemberKind::Method => {
                Some(member)
            }
            _ => None,
        },
    )
}

pub(crate) fn scoped_receiver_methods<'a>(
    workspace: &'a Workspace,
    file: &Path,
    scope: &Path,
    name: &str,
) -> Vec<(&'a PathBuf, &'a Symbol)> {
    receiver_selection(
        workspace,
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
        |member| match member {
            ReceiverMember::Method(path, symbol) => Some((path, symbol)),
            ReceiverMember::Declaration(_) => None,
        },
    )
}

pub(crate) fn receiver_members<'a>(
    workspace: &'a Workspace,
    file: &Path,
    name: &str,
) -> Vec<ReceiverMember<'a>> {
    scoped_receiver_members(workspace, file, file, name)
}

pub(crate) fn scoped_receiver_members<'a>(
    workspace: &'a Workspace,
    file: &Path,
    scope: &Path,
    name: &str,
) -> Vec<ReceiverMember<'a>> {
    receiver_selection(
        workspace,
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

pub(crate) fn receiver_member<'a>(
    workspace: &'a Workspace,
    file: &Path,
    receiver: &str,
    name: &str,
) -> Option<ReceiverMember<'a>> {
    scoped_receiver_member(workspace, file, file, receiver, name)
}

pub(crate) fn scoped_receiver_member<'a>(
    workspace: &'a Workspace,
    file: &Path,
    scope: &Path,
    receiver: &str,
    name: &str,
) -> Option<ReceiverMember<'a>> {
    receiver_selection(
        workspace,
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

fn interface_members<'a>(
    workspace: &'a Workspace,
    consumer: Option<&Path>,
    file: &'a PathBuf,
    owner: &'a Symbol,
) -> Option<Vec<MemberDeclaration<'a>>> {
    let mut pending = vec![(file, owner)];
    let mut seen = HashSet::new();
    let mut methods = BTreeMap::new();
    while let Some((file, owner)) = pending.pop() {
        if !seen.insert((file, owner.start)) {
            continue;
        }
        if seen.len() > 4096 {
            return None;
        }
        let info = owner
            .type_info
            .as_ref()
            .filter(|info| info.is_interface())?;
        let same = consumer.is_none_or(|consumer| {
            file.parent() == consumer.parent()
                && workspace.files[file].package == workspace.files[consumer].package
        });
        for name in &info.members {
            if same || resolve::exported(name) {
                methods
                    .entry(member_id(workspace, file, name))
                    .or_insert(MemberDeclaration {
                        file,
                        owner,
                        name,
                        kind: MemberKind::Method,
                        embedded: false,
                    });
            }
        }
        // Source-order depth first: complete the first embedded interface's
        // method set before merging the next, independent of struct depth.
        for embedded in info.embedded.iter().rev() {
            if let Some((file, owner)) = underlying_target(workspace, file, embedded)
                && owner
                    .type_info
                    .as_ref()
                    .is_some_and(|info| info.is_interface())
            {
                pending.push((file, owner));
                if pending.len() > 4096 {
                    return None;
                }
            }
        }
    }
    Some(methods.into_values().collect())
}

fn receiver_selection<'a, T>(
    workspace: &'a Workspace,
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
    let Some((root, ty)) = selection
        .root
        .or_else(|| embedded_target(workspace, scope, name))
    else {
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
                    && workspace.files[path].package == workspace.files[file].package;
            let shape = if ty
                .type_info
                .as_ref()
                .is_some_and(|info| info.target.is_some())
            {
                underlying_target(workspace, scope, spelling)
            } else {
                Some((path, ty))
            };
            let fallback;
            let methods = if let Some(index) = selection.methods {
                index.get(workspace, path, &ty.name)
            } else {
                fallback = workspace.symbols(path);
                &fallback
            };
            for &(method_path, method) in methods {
                if matches(&method.name)
                    && method.receiver.as_deref() == Some(ty.name.as_str())
                    && (matches!(selection.visibility, Visibility::Implementation)
                        || workspace.selector_file_visible(file, method_path))
                    && (same || resolve::exported(&method.name))
                    && !shape.is_some_and(|(_, shape)| {
                        shape
                            .type_info
                            .as_ref()
                            .is_some_and(|info| info.is_interface())
                    })
                {
                    names
                        .entry(member_id(workspace, method_path, &method.name))
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
                    let Some(methods) = interface_members(workspace, consumer, field_path, owner)
                    else {
                        return Vec::new();
                    };
                    for member in methods {
                        if matches(member.name) {
                            names
                                .entry(member_id(workspace, member.file, member.name))
                                .and_modify(|slot| *slot = None)
                                .or_insert_with(|| select(ReceiverMember::Declaration(member)));
                        }
                    }
                    continue;
                }
                let same = matches!(selection.visibility, Visibility::Implementation)
                    || field_path.parent() == file.parent()
                        && workspace.files[field_path].package == workspace.files[file].package;
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
                            .entry(member_id(workspace, field_path, member))
                            .and_modify(|slot| *slot = None)
                            .or_insert_with(|| select(ReceiverMember::Declaration(declaration)));
                    }
                }
                for embedded in &info.embedded {
                    let field = crate::analysis::scope::named_root(embedded)
                        .rsplit('.')
                        .next()
                        .expect("embedded name");
                    if matches(field) && (same || resolve::exported(field)) && !info.is_interface()
                    {
                        let declaration = MemberDeclaration {
                            file: field_path,
                            owner,
                            name: field,
                            kind: MemberKind::Field,
                            embedded: true,
                        };
                        names
                            .entry(member_id(workspace, field_path, field))
                            .and_modify(|slot| *slot = None)
                            .or_insert_with(|| select(ReceiverMember::Declaration(declaration)));
                    }
                    if let Some((base_path, base)) =
                        embedded_target(workspace, field_path, embedded)
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
