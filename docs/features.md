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
- External-edit freshness: when the client supports dynamic registration (Zed
  does), the server watches `**/*.go`, `**/go.mod`, `**/go.sum` and `**/go.work`.
  A watched batch reindexes changed/created/deleted files with one dependency and
  SDK discovery pass, refreshes the module plan for manifest events, and
  republishes diagnostics for open documents in the changed package and its
  direct importers. When the client/agent produces no editor events at all, a
  bounded disk poller (`initializationOptions.pollIntervalMs`, else
  `GOQUERY_POLL_MS`, default 1000 ms) detects direct edits, new files and
  manifest changes; after one detection it polls every 100 ms for 3 s so a
  multi-file agent burst is not spread across full intervals. In workspaces of at
  most 512 poll-scope files (configurable with `GOQUERY_ONDEMAND_MAX_FILES`), the
  first semantic request after ~300 ms of idle time also polls on demand, so an
  agent's immediate post-write query sees the edit without waiting for the base
  interval; larger trees keep the background schedule to avoid a full-tree scan
  per query. Clean open buffers are reloaded from disk while dirty editor buffers
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
  disk polling for external (agent/shell) edits; disabling polling limits those
  buffers to refreshing when opened, changed or saved. Diagnostic pushes after
  one mutation are capped at 64 open documents (changed package first, then
  direct importers).
- Completion ranking approximates gopls but lacks expected-type weighting and
  import suggestions.
- Semantic classification is preliminary and not a Go type checker; results for
  unsupported constructs stay conservative.
