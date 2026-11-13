//! Request-local implementation relations for types and methods.
//!
//! The type relation itself stays in `hierarchy::relations`; this module only
//! resolves the cursor object (type name or method name), chooses the direction
//! from interface concreteness, and projects bounded declaration locations.
//! Method queries reuse the same relation and map every related type to the
//! corresponding member of its method set.
use super::hierarchy::SymbolTarget;
use super::{Context, SourceLocation, SourceRange};
use crate::analysis::member::MemberDeclaration;
use crate::analysis::method_set::ReceiverMember;
use crate::analysis::scope::{self, DeclarationKind, Identity, Resolver};
use crate::analysis::source::{Name, Role, SourceAnalysis};
use crate::index::{IndexedSymbol as Symbol, SymbolKind};
use crate::syntax::MemberKind;
use crate::text::SourceMap;
use crate::workspace::Workspace;
use std::path::Path;

/// Result rows are bounded exactly like the structural hierarchy relations.
const MAX_RESULTS: usize = 1024;

/// A cursor query resolved to one of the two implementation relations.
enum Target<'a> {
    /// Concrete implementation types of an interface, or interfaces a concrete
    /// type implements, depending on the declaration's form.
    Type { file: &'a Path, symbol: &'a Symbol },
    /// Implementations of a method, or interface methods implemented by one.
    Method {
        file: &'a Path,
        owner: &'a Symbol,
        name: &'a str,
    },
    /// Queries that cannot produce a relation, e.g. predeclared types without
    /// methods. Gopls answers these with a null result.
    Empty,
}

/// A protocol result row: a workspace declaration, or the universe `error`
/// declaration that gopls resolves from the pinned SDK.
pub(crate) enum ImplementationRow<'a> {
    Location(SourceLocation<'a>),
    Builtin(&'static str),
}

impl<'a> Context<'a> {
    /// Implementation targets in gopls order, or `None` for a null result.
    pub(crate) fn implementation_locations(
        &self,
        offset: usize,
    ) -> Result<Option<Vec<ImplementationRow<'a>>>, (i32, String)> {
        let source = self.source();
        if let Some((callee, selector)) = call_position(source, offset) {
            let static_call = selector
                || self.workspace.files.get(self.file()).is_some_and(|stub| {
                    stub.symbols.iter().any(|symbol| {
                        symbol.name == callee && (symbol.is_type() || symbol.is_function())
                    })
                });
            if static_call {
                return Err((0, "not a dynamic function call".to_owned()));
            }
        }
        let Some(selection) = crate::analysis::source::identifier_at(source, offset) else {
            return Err((0, "no identifier found".to_owned()));
        };
        let file = self.file();
        let stub = self
            .workspace
            .files
            .get(file)
            .ok_or((-32801, "implementation file is unavailable".to_owned()))?;
        let target = if let Some(symbol) = stub
            .symbols
            .iter()
            .find(|symbol| symbol.start == selection.start && symbol.end == selection.end)
        {
            classify_symbol(self.workspace, file, symbol)?
        } else if let Some(member) = self
            .workspace
            .member_declaration_target(file, source, &selection, &SourceMap::new(source))
            // Implementation queries on an embedding follow its written type;
            // highlight queries use the implicit field declaration instead.
            .filter(|member| !member.embedded)
        {
            classify_member(member)?
        } else {
            let analysis = SourceAnalysis::new(source, stub)?;
            let Some(name) = analysis.names.iter().find(|name| name.span == selection) else {
                return Err((0, "no identifier found".to_owned()));
            };
            let identity = Resolver::new(self.workspace, file).resolve(&analysis, source, name);
            classify_identity(self.workspace, file, source, &analysis, name, identity)?
        };
        let mut builtin = None;
        let mut locations = match target {
            Target::Empty => Vec::new(),
            Target::Type { file, symbol } => {
                let supers = !self.type_is_interface(file, symbol);
                if supers
                    && crate::analysis::query::hierarchy::relations::implements_builtin_error(
                        self.workspace,
                        file,
                        symbol,
                    )
                {
                    builtin = Some("error");
                }
                self.type_relations(file, symbol, supers)?
                    .into_iter()
                    .map(declaration_location)
                    .collect()
            }
            Target::Method { file, owner, name } => {
                let supers = !self.type_is_interface(file, owner);
                if supers
                    && crate::analysis::query::hierarchy::relations::implements_builtin_error(
                        self.workspace,
                        file,
                        owner,
                    )
                {
                    // gopls reports the universe error type for any method query
                    // on a type that satisfies it, not only for Error itself.
                    builtin = Some("error");
                }
                // The queried method itself is not an implementation of itself;
                // gopls skips the object at the query position.
                self.method_relation_locations(file, owner, name)?
            }
        };
        locations = bounded_sorted(locations);
        let mut rows: Vec<ImplementationRow<'a>> = locations
            .into_iter()
            .map(ImplementationRow::Location)
            .collect();
        if let Some(name) = builtin {
            rows.push(ImplementationRow::Builtin(name));
        }
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(rows))
    }

    fn method_relation_locations(
        &self,
        file: &'a Path,
        owner: &'a Symbol,
        name: &'a str,
    ) -> Result<Vec<SourceLocation<'a>>, (i32, String)> {
        let supers = !self.type_is_interface(file, owner);
        let required = [name];
        let mut locations: Vec<SourceLocation<'a>> = Vec::new();
        for target in self.type_relations(file, owner, supers)? {
            let Some(spelling) = self
                .workspace
                .implementation_spelling(target.file, target.symbol)
            else {
                continue;
            };
            let root = self
                .workspace
                .files
                .get_key_value(target.file)
                .map(|(path, _)| (path, target.symbol));
            let members = self.workspace.implementation_members(
                target.file,
                &spelling,
                Some(&required),
                None,
                root,
            );
            for member in members {
                if !self.member_matches(member, file, name) {
                    continue;
                }
                // A promoted method declared by the queried interface keeps the
                // interface declaration as its location; gopls reports it and
                // dedups equal locations across embedding types.
                if let Some(location) = self.member_location(member) {
                    locations.push(location);
                }
                break;
            }
        }
        Ok(locations)
    }

    /// Unexported method names carry the declaring package's identity.
    fn member_matches(&self, member: ReceiverMember<'a>, file: &Path, name: &str) -> bool {
        let (origin, member_name) = match member {
            ReceiverMember::Method(origin, symbol) => (origin.as_path(), symbol.name.as_str()),
            ReceiverMember::Declaration(declaration) => {
                (declaration.file.as_path(), declaration.name)
            }
        };
        member_name == name
            && self.workspace.member_id(origin, member_name) == self.workspace.member_id(file, name)
    }

    fn member_location(&self, member: ReceiverMember<'a>) -> Option<SourceLocation<'a>> {
        match member {
            ReceiverMember::Method(file, symbol) => Some(SourceLocation {
                file: file.as_path(),
                range: selection_range(&symbol.selection),
            }),
            ReceiverMember::Declaration(declaration) => {
                let data = self.workspace.member_data(declaration)?;
                Some(SourceLocation {
                    file: declaration.file.as_path(),
                    range: selection_range(&data.selection),
                })
            }
        }
    }
}

fn declaration_location(target: SymbolTarget<'_>) -> SourceLocation<'_> {
    SourceLocation {
        file: target.file,
        range: selection_range(&target.symbol.selection),
    }
}

fn selection_range(range: &tower_lsp::lsp_types::Range) -> SourceRange {
    SourceRange::coordinates(
        (range.start.line, range.start.character),
        (range.end.line, range.end.character),
    )
}

/// Results keep the protocol's URI ordering, deduplicated within the cap.
fn bounded_sorted<'a>(rows: Vec<SourceLocation<'a>>) -> Vec<SourceLocation<'a>> {
    let mut rows: Vec<_> = rows
        .into_iter()
        .map(|row| {
            let key = (
                crate::workspace::uri(row.file),
                row.range.start.line,
                row.range.start.utf16_column,
                row.range.end.line,
                row.range.end.utf16_column,
            );
            (key, row)
        })
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows.dedup_by(|left, right| left.0 == right.0);
    rows.truncate(MAX_RESULTS);
    rows.into_iter().map(|(_, row)| row).collect()
}

/// gopls' `implFuncs` treats a cursor on a call's `(` token (or the character
/// immediately after it) as a signature query. A static function, method or
/// conversion callee cannot be a dynamic function call, so gopls answers with
/// this request error instead of a method-set result.
fn call_position(source: &str, offset: usize) -> Option<(&str, bool)> {
    let bytes = source.as_bytes();
    let lparen = match bytes.get(offset) {
        Some(&b'(') => offset,
        Some(_) if offset > 0 && bytes.get(offset - 1) == Some(&b'(') => offset - 1,
        _ => return None,
    };
    let mut end = lparen;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    if start == end || bytes[start].is_ascii_digit() {
        return None;
    }
    let mut dot = start;
    while dot > 0 && bytes[dot - 1].is_ascii_whitespace() {
        dot -= 1;
    }
    Some((&source[start..end], dot > 0 && bytes[dot - 1] == b'.'))
}

fn classify_symbol<'a>(
    workspace: &'a Workspace,
    file: &'a Path,
    symbol: &'a Symbol,
) -> Result<Target<'a>, (i32, String)> {
    match symbol.kind {
        SymbolKind::Type => Ok(Target::Type { file, symbol }),
        SymbolKind::Method => {
            let Some(receiver) = symbol.receiver.as_deref() else {
                return Ok(Target::Empty);
            };
            let Some(name) = scope::named_type(receiver) else {
                return Ok(Target::Empty);
            };
            let Some((owner_file, owner)) =
                workspace.type_declaration(file, scope::named_root(&name))
            else {
                return Ok(Target::Empty);
            };
            Ok(Target::Method {
                file: owner_file,
                owner,
                name: symbol.name.as_str(),
            })
        }
        SymbolKind::Function => Err((
            0,
            format!(
                "{} is a function, not a method (query at 'func' token to find matching signatures)",
                symbol.name
            ),
        )),
        SymbolKind::Variable => Err((0, format!("{} is a var, not a type", symbol.name))),
        SymbolKind::Constant => Err((0, format!("{} is a const, not a type", symbol.name))),
    }
}

fn classify_member(member: MemberDeclaration<'_>) -> Result<Target<'_>, (i32, String)> {
    if member.kind == MemberKind::Method {
        Ok(Target::Method {
            file: member.file,
            owner: member.owner,
            name: member.name,
        })
    } else {
        Err((0, format!("{} is a var, not a type", member.name)))
    }
}

fn classify_identity<'a>(
    workspace: &'a Workspace,
    file: &'a Path,
    source: &str,
    analysis: &SourceAnalysis,
    name: &Name,
    identity: Option<Identity<'a>>,
) -> Result<Target<'a>, (i32, String)> {
    let spelling = &source[name.span.clone()];
    match identity {
        Some(Identity::Global(path, symbol)) => classify_symbol(workspace, path, symbol),
        Some(Identity::Member(member)) => classify_member(member),
        Some(Identity::Local(selection)) => {
            if let Some(binding) = analysis
                .locals
                .definition_at(&selection, spelling)
                .filter(|binding| binding.selection == selection)
            {
                return match binding.declaration {
                    DeclarationKind::Constant => {
                        Err((0, format!("{spelling} is a const, not a type")))
                    }
                    DeclarationKind::Value | DeclarationKind::Variadic => {
                        Err((0, format!("{spelling} is a var, not a type")))
                    }
                    DeclarationKind::Type | DeclarationKind::TypeParameter => Ok(Target::Empty),
                };
            }
            if matches!(name.role, Role::Parameter | Role::Result) {
                Err((0, format!("{spelling} is a var, not a type")))
            } else {
                Ok(Target::Empty)
            }
        }
        None => classify_unresolved(workspace, file, name, spelling),
    }
}

/// Unresolved spellings still have gopls object categories: builtins, package
/// names and the nil-object group. Error text is part of the observed protocol.
fn classify_unresolved<'a>(
    workspace: &'a Workspace,
    file: &Path,
    name: &Name,
    spelling: &str,
) -> Result<Target<'a>, (i32, String)> {
    if scope::universe::imported_name(workspace, file, spelling) {
        return Err((0, format!("{spelling} is a pkgname, not a type")));
    }
    if scope::universe::builtin_type(spelling) {
        // The interface `error` has a non-empty method set in gopls; its
        // implementations require the universe declaration, which this
        // metadata query cannot synthesize yet.
        return Ok(Target::Empty);
    }
    if scope::universe::builtin_function(spelling) {
        return Err((0, format!("{spelling} is a builtin, not a type")));
    }
    match spelling {
        "nil" => Err((0, "nil is a nil, not a type".to_owned())),
        "true" | "false" | "iota" => Err((0, format!("{spelling} is a const, not a type"))),
        _ if matches!(name.role, Role::Field | Role::FieldDeclaration(_)) => {
            Err((0, format!("{spelling} is a var, not a type")))
        }
        _ => Err((0, format!("{spelling} denotes unknown object"))),
    }
}
