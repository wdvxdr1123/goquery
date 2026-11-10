# Features

- Navigation: definition, declaration, type definition, references,
  implementation and document highlights from resolved symbol identity.
- Symbols: document symbols (flat and hierarchical), and fuzzy workspace symbol
  search with gopls-style query syntax (`^prefix`, `'substring`, `suffix$`).
- Completion and signature help: gopls-derived scoring, scoped locals and
  parameters, snippets, documentation and variadic signatures.
- Hover with Go types, signatures and doc comments rendered like gopls, including
  the pkg.go.dev footer.
- Rename and prepare-rename with scope/shadowing checks and versioned workspace
  edits across identifiers, imports, fields, methods, receivers, labels and doc
  comments/links.
- Call hierarchy (incoming/outgoing) and type hierarchy by structural method sets,
  with separate `goquery/embedding/{supertypes,subtypes}` extensions for explicit
  named embedding.
- Diagnostics for compiler/syntax-class errors, with quick fixes and
  `source.organizeImports` edits.
- Initialization runs `go env` once for SDK and module-cache defaults; formatting
  runs `gofmt`. Both invocations are bounded, and a failure falls back instead of
  failing the request.
- Semantic tokens (full and range), folding ranges, code lenses (`//go:generate`,
  cgo regeneration) and document links (pkg.go.dev and comment/string URLs).
- Workspace folders with dynamic refresh; open buffers override disk contents and
  closing restores the saved declarations.
- Module awareness: `go.mod`/`go.work`, local `replace` directives, an offline
  module cache with bounded MVS, SDK discovery, build constraints, and Go
  source-checkout `std`/`cmd` views.

## Known gaps

- Full type/expression resolution, generic call inference and complete
  interface/type-set semantics are unfinished. Unsupported facts return unknown
  instead of a guess.
- Complete build selection, vendor mode, dependency downloads and checksums are
  not implemented; the module cache must be prepared offline.
- No inlay hints, and no gopls analyzer diagnostics: diagnostics cover
  compiler/syntax-class errors only.
- Result caps: 4,096 reference locations (an error beyond that), 200 completions,
  100 workspace symbols; files above 15 MiB are not indexed.
- Requests are handled sequentially; cancellation of an in-flight synchronous
  query is incomplete; documents synchronize in full, not incrementally.
- Completion ranking approximates gopls but lacks expected-type weighting and
  import suggestions.
- Semantic classification is preliminary and not a Go type checker; results for
  unsupported constructs stay conservative.
