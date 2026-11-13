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

## Expanded SDK admission and promoted-field reads

The expanded SDK `Link` discrepancy is now traced to the existing additional
module view's 16 MiB cumulative source admission budget. Opening the compiler
sample first admitted 330 files / 16,757,093 source bytes; after opening the
linker sample, totals reached 337 files / 16,776,549 bytes. The loader's deeper
`ErrorReporter` declaration could not be admitted. The missing
`AfterErrorAction` is therefore an incomplete dependency view, rather than a
field-shadowing discrepancy. Debug evidence is retained locally in
`benchmarks/promotion-debug.log`; temporary instrumentation was removed.

Additional-view admission now logs exhaustion rather than silently concealing
this limit. The existing budgets remain unchanged. Promoted-field collection
also directly consumes the shared struct-field parser's result: the redundant
second full-file read, declaration lexing and copied field inventory have been
removed. This reduces simultaneous query-local source storage and duplicated
work without adding caches or changing field resolution rules. A general
solution for prioritizing dependencies within the admission budget is still
unfinished; do not count this cleanup as resolving the SDK residual.

After the cleanup, complete release hover responses are unchanged row by row:
standard library 91/99, Hugo 51/51 and Ollama 58/59. No new mismatches or changed
goquery result bodies appeared in those samples, and no sampled hover exceeded
100 ms. Reports: `benchmarks/promoted-read-{stdlib,hugo,ollama}-hover.json`.
These comparisons ran concurrently with the memory workload, so they are not
isolated latency benchmarks. Full Rust tests, formatting and Clippy passed.

The final release passed the Ollama 1068-file full session again (one cycle,
type definitions and full type shapes, 47 feature consistency responses): root
peak working set 58,236,928 bytes, sampled private bytes 48,181,248. Go children
are excluded from the root metrics. Report:
`benchmarks/promoted-read-ollama-session.json`. This confirms the existing memory
gate; these independent samples do not establish a measured peak reduction.

## Hugo feature matrix after dependency preparation

All 15 feature plans were replayed with `--limit 3` against the pinned clean Hugo
checkout and prepared offline module graph. Each comparison retains complete
responses; declarations use gopls definition as their documented oracle.

| Feature | Matched/total |
| --- | --- |
| Hover | 51/51 |
| Rename | 46/46 |
| Implementation | 14/17 |
| Document highlight | 33/36 |
| Flat document symbols | 3/3 |
| Document links | 3/3 |
| Code lenses | 3/3 |
| Formatting | 3/3 |
| Compiler/syntax diagnostics | 5/5 (opened files 3/3) |
| Code actions | 6/6 |
| Definition | 9/9 |
| Declaration | 31/31 |
| Semantic tokens | 3/3 |
| Range semantic tokens | 3/3 |
| Folding ranges | 3/3 |

Reports are under `benchmarks/promoted-read-hugo-matrix/`, with hover separately
in `benchmarks/promoted-read-hugo-hover.json`. This is bounded plan coverage, not
complete project parity or full-build evidence. Three implementation queries on
`pageCommon` each report the same extra `resource.ResourceError` interface at
resources/resource/resourcetypes.go:66 (49 locations versus gopls' 48).
Investigate the omitted universe `error` embedding in metadata method sets; the
typed arena already models that builtin, so reuse its semantics rather than
inventing a name-based relation. The three highlight discrepancies are range-
identical: embedded `GitInfoProvider`, `SiteProvider` and `MediaTypeProvider`
references are Text (1) versus gopls Read (2).

The groups ran concurrently with the memory workload. Implementation requests in
hugolib/page__common.go exceeded 100 ms at pageCommon@1241 (310.3 ms),
getNextPrev@969 (118.1), getNextPrevInSection@1112 (136.0), getNextPrev@1017
(383.4), getNextPrevInSection@1169 (254.6), Store@2758 (260.0), Scratch@2846
(261.6), pageCommon@1005 (238.4) and pageCommon@1157 (227.5). Other sampled
feature requests stayed below 100 ms.

An isolated implementation rerun confirmed 14/17 with the same three residuals
(`benchmarks/promoted-read-hugo-implementation-isolated.json`). Slow requests
remain in hugolib/page__common.go: pageCommon@1241 (185.9 ms),
getNextPrev@1017 (178.9), getNextPrevInSection@1169 (182.9), Store@2758
(180.0), Scratch@2846 (185.8), pageCommon@1005 (183.6) and pageCommon@1157
(182.9). These exceed the latency target even without concurrent corpora.

## Embedded field identities and universe method requirements

Struct embedding declaration lookup now includes the implicit field identified
by the final type name, including qualified, pointer and instantiated forms.
Interface embeddings remain type references. Member location calculation is
shared with member-data queries, so highlight identities and navigation use the
same selection rather than duplicating token rules. Stdio e2e covers embedded
interface fields and their selector uses, pointer fields and generic fields.
Implementation cursors on an embedding follow the written type reference,
whereas highlighting uses its implicit field identity. A pinned gopls comparison
of ordinary/pointer embeddings matched all six complete implementation,
highlight and definition responses (`benchmarks/embedded-cursor-comparison.json`).

Implementation matching now checks embedded/indirect interface metadata against
the existing typed arena's complete method inventory. If indexed requirements
omit a universe method, the shared typed assignment proof checks the complete
requirement, including signature and pointer method-set rules. It does not infer
implementation from the spelling `Error`. Stdio e2e covers no Error method,
wrong Error result type and a correct pointer receiver implementation. All type
graphs remain request-local and use existing arena budgets. The Go/types oracle
remains 1153 body matches / 1 unknown and 1 type-declaration match / 5 unknowns,
with zero incorrect, rejected or unverified results
(`benchmarks/embedded-universe-type-oracle.json`).

This supplements the metadata relation, not a complete migration to typed method
sets. Pure universe-only interface metadata, generic interface instantiation and
unknown typed projections still need broader coverage. Do not treat these
targeted checks as complete interface/implementation support.

The final release snapshot is `target/embedded-universe-final.exe`; differential
reports use this fixed binary so later builds cannot change their provenance.
The final Hugo sample matches implementation 17/17 (previously 14/17), highlight
36/36 (previously 33/36), definition 9/9 and declaration 31/31. Final primary SDK
samples match implementation 24/24 and highlight 72/72. Full Rust tests,
`cargo fmt --check`, Clippy with warnings denied and the release build passed;
new regression coverage is confined to the existing stdio e2e suite.

Final complete implementation/highlight response comparisons:

| Corpus | Implementation | Document highlight |
| --- | --- | --- |
| Standard library (`--limit 6`) | 24/24 | 72/72 |
| Hugo (`--limit 3`) | 17/17 | 36/36 |
| Tile38 (`--limit 3`) | 9/9 | 25/25 |
| Ollama (`--limit 3`) | 19/19 | 36/36 |
| typescript-go (`--limit 3`) | 8/8 | 36/36 |
| Prometheus (`--limit 3`) | 26/26 | 36/36 |

Hugo rename also remains 46/46. Reports and grouped summaries are under
`benchmarks/embedded-universe-final/`. The sampled response agreement does not
establish full corpus parity. The groups ran concurrently with the memory
session; every observed request over 100 ms is listed in
[semantic-latency.md](semantic-latency.md). Implementation performance remains
unfinished.

The final snapshot passed the Ollama 1068-file open/edit/restore/close session
(one cycle, full type shapes and type definitions, 47 feature consistency
responses): root peak working set 58,392,576 bytes and sampled private bytes
48,058,368. Go children are excluded from root metrics. Report:
`benchmarks/embedded-universe-final-ollama-session.json`. This is memory-gate
evidence for the finite workload, not a claim of a measured peak reduction.

## Nongeneric implementation signature environments

The phase profile led to removal of a redundant embedding-graph traversal for
members whose declaring owner is nongeneric. Method lookup already selected the
declaration identity. Such an owner has no formal parameters to substitute, and
its signature continues to resolve in its own file scope. Generic declaring
owners still recover the instantiated environment, including renamed receiver
parameters. The change reuses existing signature/type machinery and adds no
cache or limit. Existing stdio goldens, including the 61 generic-hierarchy
relation requests, passed with the full Rust suite; fmt, Clippy and release
build also passed. No duplicate tests were added for this performance change.

The isolated four-run A/B retained identical full responses for all 17 Hugo
requests. Seven supertype queries improved from roughly 179–190 ms to 87–95 ms
(two samples per binary); another subtype query still had a 131 ms median.
Exact times and the limits of this measurement are recorded in
[semantic-latency.md](semantic-latency.md). This does not establish full latency
or semantic acceptance for the corpora.

Final gopls implementation comparisons with the fixed
`target/signature-owner-final.exe` snapshot matched all selected responses:
standard library 24/24 (`--limit 6`); Hugo 17/17, Tile38 9/9, Ollama 19/19,
typescript-go 8/8 and Prometheus 26/26 (`--limit 3` each). Reports are under
`benchmarks/signature-owner/`. All sampled external requests stayed below
100 ms in this concurrent comparison; SDK gcm_nonces.go:Overhead@3438 took
146.3 ms. The unsettled A/B timing and the settled comparison timing are
different workloads, not interchangeable latency claims.

The final snapshot also passed the Ollama 1068-file full session (one cycle,
full type shapes and type definitions, 47 feature consistency responses): root
peak working set 58,437,632 bytes and sampled private bytes 49,176,576. Go
children are excluded. Report: `benchmarks/signature-owner-ollama-session.json`.
Both remain below 100,000,000 bytes; no peak-memory reduction is claimed from
these independent samples.

A second isolated A/B enabled the harness's notification quiet wait before
queries: 17/17 complete responses stayed identical in all four alternating
runs. The seven previously slow supertype queries improved from 184–190 ms to
84–91 ms (medians of two replays each). All candidate requests in those settled
replays stayed below 100 ms, maximum 94.4 ms. The formerly slow subtype sample
was 61.7 ms after settling, so the earlier post-open timing should not be
classified as a steady-query algorithm bottleneck without further evidence.
Report: `benchmarks/signature-owner-hugo-ab-settled.json`. This finite-plan
result does not establish broader corpus or tail-latency acceptance.

## Inferred array literal extents

`analysis::check::composite` now computes `[...]T{...}` lengths from keyed
positions and implicit successors, rather than counting elements. It reuses the
native constant operands, target-sized index checks, lexical annotation resolver
and type arena. Duplicate, negative, nonconstant and overflowing indices remain
unknown. The bounded index inventory has at most 256 entries; initializer text
is limited to the existing 16 KiB query ceiling, with no allocation proportional
to the inferred array size. Package hover and `[len(variable)]` formatting share
the same helper. Syntactic fallback no longer publishes `[...]` as a resolved
type when semantic inference fails.

Stdio e2e covers imported typed keys, package `iota`, sparse/reversed keys,
untyped integral floating keys, empty arrays, local constants/types, reused
lengths and invalid/budget-exhausted cases. Existing Go/types oracle fixtures
also cover generic elements, pointers, anonymous structs, local defined element
identities and builtin `len`. Structured binding comparison now reports 1162
body matches (9 added), 1 body unknown, 1 type-declaration match and 5
type-declaration unknowns, with zero incorrect/rejected/unverified results
(`benchmarks/inferred-array-type-oracle.json`). Full Rust tests, formatting,
Clippy with warnings denied and release build passed.

The expanded SDK hover sample improves from 91/99 to 92/99: ppc64's keyed
`blockJump` now renders `[114]struct{...}`, matching gopls instead of `[...]`.
The remaining seven rows are retained. General constant array-length expressions
and full composite-value validation are still unfinished; do not classify the
extent calculation as a complete composite checker.

Final release hover comparisons (`target/inferred-array-final.exe`, reports
under `benchmarks/inferred-array/`): standard library 92/99 (`--limit 6`),
Hugo 51/51, Tile38 36/36, Ollama 58/59, typescript-go 44/44 and Prometheus
49/50 (`--limit 3` for each external project). The expanded Prometheus sample
retains an extra test-only `assertInvariants` method in the writeJobQueue hover.
Ollama retains the dependency-error `err` hover. Definition remains SDK 18/18
and Hugo 9/9; Hugo semantic tokens 3/3 and implementation 17/17 also match.
The broader SDK token sample is only 3/6: internal/trace/trace_test.go,
net/http/httptest/server.go and os/root_test.go retain differences. Those token
responses and the Prometheus discrepancy were replayed on the prior fixed
`signature-owner-final.exe` baseline: result bodies are unchanged, with no
newly failed rows. Reports:
`benchmarks/inferred-array-baseline-{semantic_tokens-stdlib,hover-prometheus}.json`.

These runs were concurrent, not isolated latency benchmarks. Requests over
100 ms were Prometheus features_test.go:Start@1380 (240.0 ms), typescript-go
quickInfoForGenericTaggedTemplateExpression_test.go:fourslash_test@154 (170.6),
Hugo page__common.go:getNextPrev@1017 (112.1) and Store@2758 (101.4), and SDK
semantic tokens for internal/trace/trace_test.go (122.3) and os/root_test.go
(218.9). Complete report plan names/paths and responses are retained.

The final snapshot passed the full Ollama 1068-file, one-cycle session (full
type definitions/shapes and 47 feature consistency responses): root peak working
set 58,216,448 bytes and sampled private bytes 48,513,024, both below
100,000,000. Report: `benchmarks/inferred-array-ollama-session.json`. Go tooling
children and the driver are excluded; this script does not sample their memory.
This is a finite root-process gate, not a claim of a measured memory reduction
or overall semantic parity.

## Method package views and doc-member semantic tokens

Hover method lookup now separates the declaration's lexical scope from the
query file's package view. The existing member-selection engine is reused via
`scoped_receiver_methods`; ordinary files/importers exclude test-only methods,
while same-package tests and same-directory external tests retain the methods
visible in their test variant. Promoted and qualified-member hovers use the
same scope split. No package/type/source cache or new resource limit was added.
Stdio e2e covers direct and promoted methods in all four contexts, including
private-method visibility.

Doc-comment semantic lookup also reuses scoped member selection rather than
searching package-level declarations for methods. This adds resolved fields and
import-qualified/promoted methods, preserving ambiguity and view rules.
Properties and methods get their respective token kinds; unresolved references
remain comments. Stdio e2e includes local/imported fields, promoted methods,
unresolved fields and test-only method links in ordinary/internal/external test
views. Existing full Rust tests, formatting and Clippy passed for these changes.

The previous SDK token residuals were decomposed from complete decoded token
responses: trace_test.go has one CombinedOutput function/method discrepancy;
httptest/server.go has 18 token-span differences from three Server.URL/Listener
field doc references; os/root_test.go has 76 differences, primarily missing
printf-directive splitting in errEndsTest wrapper calls. The latter two causes
are distinct from package-view filtering and must not be conflated.

The focused test-view fixture also exposed an existing single-line declaration
formatting gap (`struct { Item }` versus gopls `struct{ Item }`). It is retained
as a full-response discrepancy rather than normalized away or removed from the
fixture. Native type-arena test-variant identity remains a broader follow-up;
the hover/member-selection fix is not a claim of complete test-view semantics.

Final snapshot: `target/test-view-scoped-final.exe`. The new doc-member path
was additionally checked against gopls for dot imports: doc references use
package scope or explicit import qualifiers, rather than expression/file-scope
dot-import names. Test-only types and globals are also excluded from production
doc references. The initial overly broad lookup was corrected before the final
snapshot; stdio e2e now covers both boundaries. The focused full-response
comparison is 7/11: four Item hovers and three complete token responses match;
the four Wrapper hovers retain only the existing single-line declaration
formatting discrepancy. Report: `benchmarks/test-method-view-scoped.json`.

Complete final corpus comparisons (`benchmarks/test-view-scoped/`):

| Corpus | Hover | Semantic tokens |
| --- | --- | --- |
| Standard library (`--limit 6`) | 92/99 | 4/6 |
| Hugo (`--limit 3`) | 51/51 | 3/3 |
| Tile38 (`--limit 3`) | 36/36 | 3/3 |
| Ollama (`--limit 3`) | 58/59 | 3/3 |
| typescript-go (`--limit 3`) | 44/44 | 3/3 |
| Prometheus (`--limit 3`) | 50/50 | 3/3 |

Prometheus's test-method hover residual is resolved (previously 49/50). SDK
semantic tokens improve from 3/6 to 4/6: httptest/server.go now matches its
complete reference response. It is the only changed SDK token result; no
previously matched row failed. The CombinedOutput and errEndsTest-formatting
residuals remain. Hugo implementation 17/17 and highlight 36/36 still match;
SDK definition remains 18/18. These bounded samples do not establish full parity.

The groups ran concurrently. Observed requests over 100 ms were Prometheus
features_test.go:Start@1380 (341.7 ms), typescript-go
findAllReferencesFromLinkTagReference3_test.go:fourslash_test@149 (125.7), SDK
tokens for trace_test.go (127.6) and root_test.go (275.6), and Hugo
page__common.go implementation requests pageCommon@1241 (113.8),
getNextPrev@1017 (169.8), getNextPrevInSection@1169 (147.3), Store@2758 (144.5),
Scratch@2846 (115.0), pageCommon@1005 (114.1) and pageCommon@1157 (115.4).
These are not isolated performance comparisons; full plan paths/times remain
in the reports.

The final snapshot passed the Ollama 1068-file one-cycle session with full type
shapes/definitions and all 47 feature response consistency checks. Root peak
working set was 58,478,592 bytes and sampled private bytes 48,324,608, below
100,000,000 bytes. Go tooling children and the driver are excluded/unmeasured.
Report: `benchmarks/test-view-scoped-ollama-session.json`. Full Rust tests,
fmt check, Clippy with warnings denied and release build passed. No new unit
tests were added, and no measured memory-reduction claim is made.

## Remaining work

- Offline module-graph enumeration now succeeds. Verify matching package scope
  beyond the current bounded Hugo feature matrix with fixed dependencies.
- Expand the Hugo implementation/highlight coverage, including universe-only
  and instantiated generic interfaces, and reduce the remaining slow
  implementation queries. Preserve exact identities and request-local budgets.
- Investigate the expanded SDK view-admission/budget residuals and Ollama's
  remaining dependency-error hover. Keep all full responses.
- Investigate the two SDK token differences, single-line type declaration
  formatting, native arena test-view identity and general constant array-length
  expressions, preserving bounded inference.
- Extend syntax/type/LSP differential coverage across all existing projects;
  these small samples do not establish overall parity.
- Run the measured Ollama memory gate for any changes affecting allocations or
  retained state. Preserve bounded request-local parsing and existing layering.

Validation performed for the first Rust fix: `cargo fmt --check`, `cargo test`,
`cargo clippy --all-targets -- -D warnings`, and `cargo build --release`.
