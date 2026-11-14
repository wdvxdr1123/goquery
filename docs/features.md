# Features

- Navigation: definition, declaration, type definition, references,
  implementation and document highlights from resolved symbol identity.
- Symbols: document symbols (flat and hierarchical), and fuzzy workspace symbol
  search with gopls-style query syntax (`^prefix`, `'substring`, `suffix$`).
- Completion and signature help: gopls-derived scoring, scoped locals and
  parameters, snippets, documentation and variadic signatures.
- Hover with Go types, signatures and doc comments rendered like gopls, including
  the pkg.go.dev footer. Method lists distinguish ordinary source, same-package
  tests, same-directory external tests and unrelated importers; test helpers
  remain in their test view rather than appearing in production hover lists.
  Generic function-call hover shares native type-argument inference and
  constraint/assignment checks with result inference, including imported calls,
  partial explicit arguments and variadic expansion within existing budgets.
  Unavailable inference retains the declaration signature.
  Type-declaration hover shares bounded syntax-based presentation across globals,
  local types and alias targets. Supported inline declarations normalize field
  layout, generic parameters and expression spacing while preserving literal
  contents. Existing multiline layout and unsupported/over-budget forms retain
  their source presentation; this is not a complete Go formatter.
- Inferred array literal extents (`[...]T{...}`) follow constant keyed indices
  and implicit successors, including imported constants and supported `iota`
  groups. The same facts feed local inference and hover/array-length rendering.
  Extent inference is bounded to 256 elements and 16 KiB of initializer text;
  unavailable/invalid indices or exhausted budgets stay unknown, without
  allocating storage proportional to a sparse index. General array-length
  expressions and full composite-element checking remain incomplete.
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
  Semantic tokens split resolved field/method doc references such as
  `[Type.Field]` and `[pkg.Type.Method]` using shared member identity, promotion,
  ambiguity and visibility checks. Unresolved member references stay comments.
  Format-string splitting and format/argument highlights use the resolved
  callable signature's last fixed parameter, including local/imported functions,
  function literals and method expressions. They share one bounded call scan:
  1024 candidate calls, 512 scope builds, 16384 argument tokens per call and
  1048576 argument-token visits per request; format literals stay under 16 KiB
  and 256 operations. One active body's expressions/locals are retained during
  the scan. Highlight queries resolve only calls enclosing the cursor. Directive
  cursors support repeated/indexed arguments, dynamic width/precision, nested
  calls and escaped/raw literals; missing arguments still highlight the directive.
- Workspace folders with dynamic refresh; open buffers override disk contents and
  closing restores the saved declarations.
- External-edit freshness: when the client supports dynamic registration (Zed
  does), the server watches `**/*.go`, `**/go.mod`, `**/go.sum` and `**/go.work`.
  A watched batch reindexes changed/created/deleted files with one dependency and
  SDK discovery pass, refreshes the module plan for manifest events, and
  republishes diagnostics for open documents in the changed package and its
  direct importers. When the client/agent produces no editor events at all, a
  file watcher (default `fileWatch: true`) triggers bounded disk reconciliation
  after a 100 ms burst debounce. There is no idle scan timer, request-triggered
  poll or polling fallback. Errors are logged. Registration budgets can be raised
  or removed with `fileWatchMaxRoots`, `fileWatchMaxDirectories` and
  `fileWatchMaxEntries` (0 removes a limit). `fileWatchExcludeDirectories` filters
  relative directory prefixes using regexes without changing initial indexing
  or editor events. `fileWatch: false` disables server watching. Clean buffers
  reload from disk while dirty editor buffers
  stay authoritative. Affected diagnostics are pushed without a stale
  document version for externally reloaded documents. **Saves** use the wider
  scope, and `didChange` widens to the changed package plus direct importers
  when the package clause or declaration header/identity changes (signature, type
  body, initializer), so unsaved cross-package edits are reflected without
  reopening files. Body-only and
  import-only edits refresh only the edited buffer. Importers are narrowed by the
  changed declaration names and capped at 16 documents per live edit; save and
  watched batches use the wider 64-document, all-importer scope. The dynamic
  watch registration is replaced when workspace folders change, so newly added
  folders are watched too. `GOQUERY_TRACE_EDITS` logs each mutation's
  affected-document count and elapsed time to stderr.
- Module awareness: `go.mod`/`go.work`, local `replace` directives, an offline
  module cache with bounded MVS, SDK discovery, build constraints, and Go
  source-checkout `std`/`cmd` views. Unsaved `go.work` buffers apply immediately;
  `go.mod` planning reads the saved/on-disk manifest. A manifest edit whose
  selected roots, module paths, main modules and checkout mode are unchanged is
  detected by a plan digest and skips the root walk and dependency/SDK reload;
  a plan-changing main-manifest edit reuses the compact stubs of unchanged roots,
  retains still-reachable dependency/SDK packages, and loads only newly reachable
  mounts (pruning unreachable ones), while nested-module manifests still force a
  full rescan.

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
- Clients without `workspace/didChangeWatchedFiles` dynamic registration rely on
  the server file watcher for external edits; disabling/unavailable watching limits those
  buffers to refreshing when opened, changed or saved. Diagnostic pushes after
  one mutation are capped at 64 open documents (changed package first, then
  direct importers).
- Completion ranking approximates gopls but lacks expected-type weighting and
  import suggestions.
- Semantic classification is preliminary and not a Go type checker; results for
  unsupported constructs stay conservative.
