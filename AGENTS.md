# Project guidance

## Notes

- Do not write invalid or duplicate tests. Keep integration tests centralized in
  `tests/stdio.rs` (real stdio e2e); put pure logic contract tests in the
  corresponding module's `#[cfg(test)]` and do not add scattered test files.
- Keep the multi-layered architecture; do not pile too many responsibilities into
  a single file. Prefer placing new logic in existing modules.
- Do not introduce unnecessary dependencies. After changes, always run
  `cargo fmt --check`, `cargo test` and `cargo clippy --all-targets -- -D warnings`.
- Use the edit/write tools for code changes; do not rewrite source files in bulk.
- Record unfinished capabilities honestly; do not redefine acceptance criteria
  around current results.

## Objective

Build a compact Rust Go language server with a ccls-like feature scope and gopls
as the semantic reference, for editors and AI-assisted development.

- The server is a library (`src/lib.rs`) plus a thin stdio binary
  (`src/main.rs` → `goquery::run`), built on tower-lsp with a current-thread
  Tokio runtime and sequential request handling.
- goquery indexes source directly and answers semantic requests without launching
  `go` or `gopls`. The only Go invocations are one bounded `go env` probe during
  initialization (SDK/module-cache defaults) and `gofmt` for formatting. Both may
  fail without failing initialization/requests; Go child processes are reported
  separately from the server root.
- Large workspaces must stay usable. For the pinned Ollama full-project workload,
  the Rust server root process must remain below 100,000,000 bytes in both OS
  peak working set and sampled private bytes; report children separately.
  Keep sources, ASTs, type graphs and caches query-local and bounded.
- Semantic results should be substantially consistent with pinned gopls
  (v0.24.0) and Go 1.27.3, using the standard library as the primary corpus and
  the pinned external projects as secondary baselines. Record remaining
  differences instead of hiding them.
- Do not replace the Rust semantic implementation with a gopls/Go proxy.

## Layout

- `src/server/`: tower-lsp lifecycle, capability negotiation, wire routing,
  custom `LspService` methods; stdout is reserved for the protocol.
- `src/features/`: typed feature dispatch and protocol result construction.
- `src/analysis/`: shared semantic algorithms, query layer (`analysis::query`)
  and the `Context` used by document features.
- `src/workspace/`: document/module ownership, initialization, module graph,
  SDK and offline dependency mounts.
- `src/index.rs`: indexed files/symbols and metadata indexes. Syntax declaration
  stubs stay in `syntax`; indexed symbols compose them without duplicating fields.
- Leaf modules at the crate root: `syntax` (Go lexer/parser/AST/view),
  `fuzzy` (gopls-derived symbol and completion matchers), `buildconstraint`
  (`//go:build` parsing/tags), `gomod` (`go.mod`/`go.work` directives and
  versions), `bloom` (fixed-storage negative filters), `text` (UTF-16/byte maps),
  `error` (shared JSON-RPC/LSP error codes).
- `src/lib.rs` exposes `pub mod syntax` and `pub mod fuzzy` (and `run`); all
  other modules are private. `tests/stdio.rs` uses `goquery::syntax` /
  `goquery::fuzzy` so the e2e suite stays centralized.
- `crates/` contains only the vendored `lsp-types` patch. Cargo patches work at
  package granularity: only `document_link.rs` differs from upstream 0.94.1 to
  keep raw document-link targets (`Option<String>`). Keep the rest byte-identical
  and follow `crates/lsp-types/README.md`.
- `tests/`: stdio e2e client, protocol goldens (`UPDATE_GOLDEN=1` regenerates)
  and fixtures, including Go oracle fixtures.

## Architecture rules

- Use tower-lsp for framing, lifecycle and typed routing; do not add a second
  protocol dispatcher. Register custom methods with `LspService`.
- Keep dependencies directed: features → analysis/query/workspace; analysis must
  not import feature or server modules. Do not reintroduce retired Workspace
  semantic entry points; document features admit through `analysis::query::Context`.
- Workspace updates enter through typed events; mutable storage belongs to
  `workspace`. Analysis must not hold locks across await points.
- Do not retain closed-file sources, ASTs or type graphs between requests. Read a
  closed source once with a bounded handle; open buffers override disk contents.
  Document excluded files as detached orphan packages with bounded sibling
  stubs; prune when the last orphan in a directory closes.
- Keep request-local scopes, arenas and memos under existing budgets; on budget
  exhaustion return unknown/empty rather than partial incorrect results.
  Unsupported or unavailable semantic facts must stay unknown, never guessed.
- Bloom/negative filters may reject candidates only when the probed key is
  necessary; positive probes still require exact semantic checks. Keep
  identifier and import-path domains separate and rebuild filters on stub edits.
- Stay aligned with go/types semantics through declaration identities, structural
  type nodes, operand modes and explicit assignment/instantiation relations.
  Do not grow spelling heuristics in place of semantic algorithms. Compare with
  the pinned offline go/types oracle; never invoke Go during semantic queries.
- Module planning is offline: `go.mod`/`go.work`, selected main modules, local
  replacements, nested modules and cache mounts only. Never download or invoke
  Go for dependency resolution. Missing manifests/cache entries are logged as
  incomplete, not guessed.
- Build constraints, GOOS/GOARCH/GOEXPERIMENT and filename variants follow the
  Go toolchain. SDK discovery uses `initializationOptions.goroot` (including
  `""` to disable), then `go env GOROOT`, `GOROOT`, and the `go` install layout.
  The module cache has no option: `go env GOMODCACHE` → `GOMODCACHE` →
  `GOPATH/pkg/mod` → `~/go/pkg/mod`.
- Use UTF-16 positions at the LSP boundary and byte offsets internally; handle
  Unicode and malformed editor input without panicking.
- Advertise only implemented capabilities. Keep logs and diagnostics on stderr;
  stdout carries only LSP framing.
- Sample `GOQUERY_TRACE_*` switches once during workspace construction. Keep new
  resources bounded and document any new limit.

## Verification

Required before completing an implementation change:

```text
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Also run the relevant differential comparisons when changing semantic behavior:

- Differential comparison uses pinned gopls v0.24.0 and Go 1.27.3 with matching
  workspace/module scope, build context, open buffers and client capabilities.
  Compare complete responses, not selected result counts. Keep per-feature
  matched/total and residual differences explicit.
- Use the standard library as primary evidence; Tile38, Ollama, typescript-go
  and prometheus/prometheus are secondary baselines. Record preparation
  separately; package-loading success is not full-build success.
- Latency: if a request exceeds ~100 ms on the external corpora, list it
  explicitly. Rename latency is lower priority than the main queries; diagnostics
  parity is informational only (compiler/syntax diagnostics remain implemented).
- Memory claims require the measured Ollama workload; functional tests alone do
  not establish them.
- Update intentional protocol goldens with `UPDATE_GOLDEN=1 cargo test` only when
  the behavior change is intended; inspect the diff.

## Workflow

- Inspect the current tree/status before editing and preserve unrelated user
  changes.
- Batch related changes; keep commits focused and use concise messages. Stage
  explicit paths; do not add secrets or build artifacts.
- Do not push without authorization. Do not introduce delegation unless the user
  explicitly asks.
- Keep project instructions and user documentation current when architecture,
  configuration or verification commands change.
