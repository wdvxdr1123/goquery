# Semantic corpus progress

This is an unfinished work log, not an acceptance report. The goal remains Go
syntax and LSP consistency against Go 1.27.3 / gopls v0.24.0, with the standard
library primary and Tile38, Ollama, typescript-go, Prometheus and Hugo secondary.
This records the implementation checkpoints and remaining differences.

## Hugo preparation

- Repository: https://github.com/gohugoio/hugo
- Pinned revision: `ef7c52509321b14b0dfce34ef04be010b37ca05d`.
- `scripts/fetch-hugo.ps1` prepares the revision, with optional module downloads
  and module-manifest enumeration. `compare_features.py` accepts `--project hugo`,
  checks revision/cleanliness and reuses existing feature plans and comparison.
- `--hierarchical-symbols` explicitly negotiates nested document symbols;
  the legacy flat response remains the default for prior measurements.
- Initial measurements had 271 unavailable cached manifests. `go mod download`
  succeeded, but offline `go list -m all` still failed. Online module-manifest
  preparation initially terminated with IPv6 timeouts to proxy.golang.org.
  Retrying with the host's existing goproxy.cn endpoint succeeded; subsequent
  `GOPROXY=off go list -m all` enumerated 431 modules and the pinned checkout
  remained clean. Full-project package loading remains unproven.
  None of this establishes full-project build success.
  Offline `go list -e -json=ImportPath,Error,DepsErrors` for `common/collections`,
  `compare` and `parser` subsequently returned three packages without Error or
  DepsErrors. This is selected-package loading evidence only, not a full build.

## First comparisons

Complete response comparisons, not selected result counts:

| Hugo feature | Matched/total |
| --- | --- |
| Folding range | 6/6 |
| Legacy document symbols | 12/12 |
| Hierarchical document symbols | 60/60 |
| Semantic tokens | 6/6 |
| Definition | 32/32 |
| Hover before trailing-comment formatting fix | 50/51 |
| Hover after fix | 51/51 |

The hover discrepancy concerned the SDK's `reflect.Value`: omission of its final
unattached comment left an extra empty line before the closing brace. A shared
helper now handles gaps after both line and block comment groups, without new
cross-request storage. Reports are under `benchmarks/hugo-*.json`; the harness
also records its own source hashes for reproducibility. Dependency preparation
changed between the initial and later runs, so these are not a controlled
performance A/B comparison. No sampled Hugo hover/definition request exceeded
100 ms in these runs. No new memory reduction or full-project memory claim is made.

## Nested-module follow-up

The nested-module sibling-import fix now seeds dependencies from admitted
same-package declarations, including already indexed dependency files, instead
of only the opened document. Existing module/directory/file/byte budgets remain.
The promoted-field inventory also includes direct declarations for shadowing,
without rendering their types or listing them as promoted rows. Real stdio e2e
covers an import-free opened file, sibling-only dependency imports, same-name
shadowing and exclusion of additional-view symbols from main workspace search.

At the nested-module checkpoint (`benchmarks/promoted-shadow-*.json`): standard
library 25/25, Hugo 51/51, Ollama 53/59. The SDK Link residual is resolved. No
request exceeded 100 ms in these samples (the three comparisons ran concurrently,
so these are not isolated latency measurements). The remaining six Ollama
differences are retained at `mlx/mlxtest/mlxtest.go`: `testThread`, `T` at three
locations, `err` and `Skipf`. The first is an unknown hover for the generic
`sync.OnceValues` function-literal initializer, where gopls resolves a callable
tuple-return signature. Do not classify this untagged source as build-excluded.

That release passed the pinned Ollama 1068-file open/edit/restore/close
session (one cycle, full type shapes and type-definition queries), including
47 feature response consistency checks. Root OS peak working set was 58,228,736
bytes and sampled private bytes 47,894,528, both below 100,000,000. Report:
`benchmarks/promoted-shadow-ollama-session.json`. Go tooling children are excluded
from these root metrics. This is the finite workload memory gate, not a claim
that arbitrary workspaces fit the same memory limit or that all queries match gopls.
Formatting, full Rust tests, Clippy and release build passed after the fixes.

## Generic callable and alias follow-up

Package-level generic call initializer hovers now reuse the existing typed
`Inference` unification/assignment path, including explicit type arguments and
function-literal arguments. Local fallbacks without a supplied lexical scope
do not use an empty environment to guess a type. The indexed generic flag routes
the query before reading its header, avoiding a duplicate closed-file header
read. No request-independent type graph or new cache is retained.

Conversion fallback now checks that an indexed callee actually names a type;
invalid `Plain[int]()` on a nongeneric function no longer fabricates a type from
bracket spelling. Stdio e2e covers inferred and explicit generic factories and
this negative case. Alias hovers expose resolved nongeneric method sets, and
exported members remain promoted through unexported embedding paths; visibility
is applied to leaf members. Cross-package e2e covers both fields and methods.
SDK version lookup also reads individual interface-method API records, such as
the `testing.TB.Skipf` addition in Go 1.2.

`scripts/compare_types.py` was still invoking the thin binary's nonexistent
oracle test. It now selects `--lib`: pinned Go/types differential reports 1153
body matches, 1 body unknown, 1 type-declaration match and 5 type-declaration
unknowns, with zero incorrect/rejected/unverified results.

Complete release hover comparisons (`benchmarks/generic-alias-*-hover.json`):

| Corpus | Matched/total |
| --- | --- |
| Standard library, expanded sample | 91/99 |
| Hugo | 51/51 |
| Ollama | 58/59 |
| Tile38 | 18/18 |
| typescript-go | 28/28 |
| Prometheus | 33/36 |

The expanded SDK sample also had eight mismatches with the earlier baseline
binary; no previously matched row became mismatched. These are new, broader
samples, not a replacement claim for complete SDK parity. Link matches in the
focused 25-query run but still lacks `AfterErrorAction` in the broader opened-file
context; view admission/budget interactions remain to investigate. The other SDK
residuals are in ppc64/ssa.go (`blockJump`, `p`), crc32_loong64.go (`p`, `crc`, two
`HasCRC32` references), and runtime/mgcpacer.go (`new`). Prometheus retains
`sample`, `samples` and `want` in util/fuzzing/fuzz_test.go. Ollama's remaining
`err` hover is unknown versus gopls' `invalid type` under dependency errors.

The comparisons ran concurrently. A typescript-go hover at
`internal/fourslash/tests/gen/goToImplementationInterface_10_test.go:fourslash_test@141`
took 149.3 ms. The isolated rerun matched 28/28 with a maximum of 44.6 ms
(`benchmarks/generic-alias-typescript-hover-isolated.json`). Other sampled requests
stayed below 100 ms. Latest Ollama 1068-file, one-cycle full session passed the memory gate:
OS root peak working set 58,470,400 bytes, sampled private bytes 48,472,064.
Report: `benchmarks/generic-alias-ollama-session.json`, including all 47 feature
consistency responses. Go tooling children are excluded from the root metrics.
Full Rust tests, fmt, Clippy and release build passed for these changes.

The architecture checker was also run. It reported two findings, both in
existing `#[cfg(test)]` code: JSON link goldens in query/links.rs and the
type-oracle identity helper in types/declaration.rs. The report is retained at
`benchmarks/generic-alias-boundaries.json`; the checker currently scans test
serialization as if it were production protocol code and has not been changed
or treated as passing. No reverse analysis dependency on features/server was
reported. Improve production/test ownership discrimination in the checker
without suppressing real production violations.

## Local builtin operands and declaration formatting

`make` and `new` type operands now reuse lexical annotation resolution, preserving
local defined-type identities. Local type hovers remove enclosing indentation
while preserving multiline raw struct-tag contents, within the existing 16 KiB
signature budget. Both behaviors have regression coverage in the stdio e2e suite.

Complete release hover comparisons now match Prometheus 36/36, up from 33/36;
the expanded standard-library sample remains 91/99. Reports are
`benchmarks/local-builtin-prometheus-hover.json` and
`benchmarks/local-builtin-stdlib-hover.json`. The Go/types oracle remains at
1153 body matches and 1 unknown, with 1 type-declaration match and 5 unknowns;
there are no incorrect, rejected or unverified results
(`benchmarks/local-builtin-type-oracle.json`). These samples do not establish
complete semantic parity.

The final release passed the Ollama 1068-file open/edit/restore/close session
(one cycle, type definitions and full type shapes, 47 feature consistency
responses). Root OS peak working set was 57,507,840 bytes and sampled private
bytes 47,276,032, below the 100,000,000-byte gate; Go children are excluded.
Report: `benchmarks/local-builtin-ollama-session.json`. Formatting, full Rust
tests, Clippy and release build passed before this measurement.

## Remaining work

- Offline module-graph enumeration now succeeds. Verify matching package scope
  and rerun the full Hugo feature matrix with fixed dependencies.
- Investigate the expanded SDK view-admission/budget residuals and Ollama's
  remaining dependency-error hover. Keep all full responses.
- Extend syntax/type/LSP differential coverage across all existing projects;
  these small samples do not establish overall parity.
- Run the measured Ollama memory gate for any changes affecting allocations or
  retained state. Preserve bounded request-local parsing and existing layering.

Validation performed for the first Rust fix: `cargo fmt --check`, `cargo test`,
`cargo clippy --all-targets -- -D warnings`, and `cargo build --release`.
