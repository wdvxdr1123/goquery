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

## Imported call-result classification checkpoint

Release snapshot: `target/declared-call-result.exe`, based on `fe6a300` plus
the uncommitted classifier change. Selector classification now projects a
resolved imported function's declared result before speculative argument
checking can consume the shared arena budget. This reuses the existing bounded
declaration-header path and declaration-file import scope, without adding a
cache or retaining sources. A result referring to a function type parameter
must still use instantiation-aware inference; parsing that formal as a
package-level same-named type would be incorrect.

With Go 1.27.3 and gopls v0.24.0, the complete SDK semantic-token sample improves
from 4/6 to 5/6 (`benchmarks/declared-call-stdlib-tokens.json`). Only
`internal/trace/trace_test.go` changes: `buildCmd.CombinedOutput` is now a method.
The other five native responses are unchanged. `os/root_test.go` still differs
in format-string splitting for the local `errEndsTest` variadic helper. SDK
hover remains 92/99 with every native response unchanged
(`benchmarks/declared-call-stdlib-hover.json`). Token requests for trace_test.go
and root_test.go took 102.3 ms and 233.2 ms respectively; no SDK hover request
exceeded 100 ms in this run. These are observations, not a controlled speed A/B.

External full-response samples (`benchmarks/declared-call/`, `--limit 3`):

| Corpus | Hover | Semantic tokens |
| --- | --- | --- |
| Hugo | 51/51 | 3/3 |
| Tile38 | 36/36 | 3/3 |
| Ollama | 58/59 | 3/3 |
| typescript-go | 44/44 | 3/3 |
| Prometheus | 50/50 | 3/3 |

The remaining Ollama dependency-error hover is unchanged. No request in these
external samples exceeded 100 ms. The focused imported/generic-call fixture
matches both complete token responses (2/2). Its hover comparison is 30/32:
the return-position formal `T` is confused with a package-level type `T`, and
the explicitly instantiated call's hover retains the generic signature instead
of displaying its substitution. Those hover gaps remain unresolved; the fixture
and complete responses are retained under `benchmarks/declared-call-fixture*`.
The existing Go/types oracle remains at 1162 body matches, one unknown, zero
incorrect/rejected/unverified; type-declaration scope is one match and five
unknowns (`benchmarks/declared-call-type-oracle.json`).

The Ollama 1068-file one-cycle open/edit/restore/close workload, with full type
shapes/definitions and all 47 feature consistency checks, passed with no failures.
Root OS peak working set was 57,946,112 bytes and sampled private bytes
47,632,384, both below 100,000,000. Driver and Go tooling children are excluded
and unmeasured. This single run does not establish a memory reduction.
Report: `benchmarks/declared-call-ollama-session.json`. Release build,
`cargo fmt --check`, `cargo test` and Clippy with warnings denied passed.

## Native generic call-hover checkpoint

Snapshot: `target/native-generic-final.exe`. Function-header hover now resolves
type-parameter uses through the parsed header's `Locals` and type-reference
spans, rather than falling through to a same-spelled package type. Imported,
indexed and parenthesized callees retain the selected identifier's own span.

Generic call hover no longer maintains a separate spelling-based unifier or
replaces identifiers across a rendered signature. It shares native actual
operands, explicit type arguments, structural unification, constraints and
assignment checks with call-result inference. Both paths obtain the checked
type argument list from the same arena operation; result annotations are parsed
with that concrete environment. The existing signature-label renderer supplies
parameter/result names and variadic labels, with its existing 16 KiB output
limit. Header input remains bounded to 16 KiB and type arguments to 32; arenas,
sources and signatures are request-local. No new cache or dependency is added.

The original imported/generic fixture improves from 30/32 to 32/32 complete hover
responses (`benchmarks/native-generic-final-fixture-hover.json`). An expanded
five-query comparison matches 5/5, covering explicit and inferred imported
calls, partial explicit arguments, named tuple results, variadic spreading and
an anonymous result whose field name and tag contain the formal's spelling.
Names and tag text stay intact while the field type is substituted. Report:
`benchmarks/native-generic-fixture/final-comparison.json`. The same boundaries
are covered in the existing stdio e2e; no new Rust unit tests were added.

The Go/types core comparison remains at 1162 body matches, one unknown, zero
incorrect/rejected/unverified, and one match/five unknowns in declaration scope
(`benchmarks/native-generic-type-oracle.json`). The SDK hover (92/99), semantic
tokens (5/6) and definition (18/18) responses remain unchanged from the preceding
checkpoint. Hugo implementation remains 17/17 and document highlights 36/36.
External hover/token samples (`--limit 3`, `benchmarks/native-generic/`) remain
Hugo 51/51 and 3/3, Tile38 36/36 and 3/3, Ollama 58/59 and 3/3, typescript-go
44/44 and 3/3, and Prometheus 50/50 and 3/3. Every sampled native response is
unchanged from the preceding checkpoint; Ollama's dependency-error hover remains
unresolved. Nine Hugo implementation requests and the SDK root_test.go token
request exceeded 100 ms during overlapping runs; all are listed in
`docs/semantic-latency.md`. Functional matches do not establish latency parity.
Unavailable generic-call inference keeps the declaration signature; this does
not establish complete generic inference, generic function-value hover or
parity for unsupported type facts.

Final Rust fmt check, full tests, Clippy with warnings denied and release build
passed. The Ollama 1068-file one-cycle session with type shapes/definitions and
all 47 feature consistency checks passed without failures. Root OS peak working
set was 58,826,752 bytes and sampled private bytes 48,177,152, below 100,000,000.
Driver and Go tooling children are excluded/unmeasured; this is a finite memory
gate, not a measured memory reduction. Report:
`benchmarks/native-generic-ollama-session.json`.

## Signature-based format tokens and argument highlights

Final snapshot: `target/signature-format-unicode.exe`. The pinned gopls v0.24.0
`formatStringAndIndex` uses a variadic signature's last fixed parameter; it
does not require an `f` suffix or inspect a wrapper body. The previous name
shortcut, first-literal search and workspace-wide same-named-method scan are
removed. Resolved function declarations supply bounded header arity; other
callables use native signature operands, keeping receiver/method-expression
arity and local shadowing. As in the reference, a defined function type itself
does not count as a direct signature for format splitting.

Token splitting and argument-identifier highlights share the call collector in
`analysis/query/highlight/format.rs`. Only one body's expressions and locals
survive between candidates; nested closures replace that scope. Limits are
1024 candidate calls, 512 scope builds, 16384 argument tokens per call and
1048576 argument-token visits per request. Literal/header input stays at 16 KiB
and format parsing at 256 operations. Exhausted/unavailable facts remain
ordinary string tokens without a guessed format association. There is no new
dependency, persistent source/AST cache or workspace-name fallback.

The shared escape-range mapper now computes the actual UTF-8 width of `\u`
code points, instead of assuming three bytes. Both `"\\x25d"` and
`"\\u0025d"` can project the directive's original source span. Multiline
strings keep gopls' ordinary per-line token behavior, while still participating
in argument highlights. The existing stdio e2e covers true/false format slots,
nonvariadic `...f` names, named/direct callable types, immediately invoked
function literals and escaped percent signs; no new unit tests were added.

The complete SDK token sample improves from 5/6 to 6/6; only os/root_test.go's
native response changes. Its errEndsTest calls now split the format parameter
instead of treating the entire literal as one token. Final native replays use
the unchanged same-plan pinned gopls responses from the preceding fresh runs;
reports retain their reference hashes and complete responses:
`benchmarks/signature-format-unicode/` (fresh reference runs under
`benchmarks/signature-format/`).

| Corpus | Semantic tokens | Document highlights |
| --- | --- | --- |
| SDK (`--limit 6`) | 6/6 | 72/72 |
| Hugo (`--limit 3`) | 3/3 | 36/36 |
| Tile38 (`--limit 3`) | 3/3 | 25/25 |
| Ollama (`--limit 3`) | 3/3 | 36/36 |
| typescript-go (`--limit 3`) | 3/3 | 36/36 |
| Prometheus (`--limit 3`) | 3/3 | 36/36 |

SDK hover remains 92/99, with its seven known residuals. The focused fixture's
two complete token responses match, including imported/local calls, method
expressions, local shadows, generic calls, function values and literals,
escaped percent signs and nested calls/closures. Report:
`benchmarks/signature-format-unicode-fixture-tokens.json`. Its highlight
comparison is explicitly 13/16: all 13 argument-cursor responses match, but
three format-directive-cursor responses still return null instead of the
reference's directive/argument pair. These failures remain retained in
`benchmarks/signature-format-fixture/unicode-highlights.json`; token parity
does not establish complete document-highlight support.

Full Rust tests, fmt check, Clippy with warnings denied and release build pass.
The final Ollama 1068-file one-cycle session with type shapes/definitions and
all 47 consistency checks passes without failures: root OS peak working set
58,114,048 bytes, sampled private bytes 49,287,168, both below 100,000,000.
Driver and Go tooling children remain excluded/unmeasured. Report:
`benchmarks/signature-format-unicode-ollama-session.json`. No comparative memory
or speed improvement is claimed. The root_test.go token request remains slow
(267.9 ms in the final replay); all >100 ms requests are recorded separately.

## Format-directive cursor highlights and scoped call resolution

Snapshot: `target/format-cursor-scoped.exe`. Cursors inside format literals and
other non-identifier call arguments now reach the shared format-association
logic, instead of returning null at the identifier-only entry point. A cursor
inside either a directive or its argument yields the same write/read pair;
missing arguments still yield the directive. Result range conversion/sorting
is shared with identity highlights. No protocol capability or dependency is
added.

The expanded comparison found and fixed two existing boundary issues: the
leading `[n]` of an indexed dynamic width was omitted from its range, and
argument-end cursors were excluded. Reference behavior keeps directive ends
exclusive, but includes argument ends. The common operation parser now retains
the original bracket position before consuming the index.

Highlight queries resolve only calls enclosing their cursor, using the same
collector as the full token scan; unrelated format calls no longer trigger
scope construction or signature queries. Existing call/scope/token-visit,
header/literal and operation limits remain in force, with one active body's
expressions/locals and no persistent AST/type/source cache.

The original three null format-cursor cases are resolved. The expanded pinned
comparison matches 257/257 complete responses, including every tested character
and end boundary in indexed/repeated directives, dynamic width/precision,
missing arguments, escaped/raw literals, Unicode/UTF-16 positions, nested calls
and argument expressions containing comments. All cases, including the initial
251/257 intermediate result, remain under
`benchmarks/signature-format-fixture/` in `cursor-comparison.json` and
`cursor-final-comparison.json`. Existing stdio e2e assertions cover directive
start/last/end positions, escaped percent spans, indexed width and argument-end
behavior. No new Rust unit tests were added.

Same-plan complete document-highlight replays against the unchanged pinned
references (`benchmarks/format-cursor/`) remain SDK 72/72, Hugo 36/36, Tile38
25/25, Ollama 36/36, typescript-go 36/36 and Prometheus 36/36, or 241/241 total.
The expanded fixture's complete token responses remain 2/2
(`benchmarks/format-cursor-fixture-tokens.json`). Two actual SDK root.Open
format-cursor responses also match fresh gopls responses in
`benchmarks/format-cursor/sdk-directives.json`.

After the other workloads finished, two fresh settled runs per binary in
alternating order compared the correct whole-file collector snapshot
`target/format-cursor-final.exe` with the scoped collector. Both complete
responses matched the pinned reference in every run. Median SDK root.Open
directive query times were 69.2/61.3 ms before and 4.7/3.5 ms after. This is
evidence for these two finite query cases, not a general latency guarantee;
the prior slow root_test.go semantic-token request remains unresolved. Report:
`benchmarks/format-cursor/sdk-scope-ab.json`. No >100 ms request appeared in
the new six-corpus highlight replays or focused comparison.

Rust fmt check, full tests, Clippy with warnings denied and release build pass.
The Ollama 1068-file one-cycle session with type shapes/definitions and all 47
consistency checks passes with no failures: root OS peak working set 58,245,120
bytes, sampled private bytes 48,279,552, both below 100,000,000. Driver/Go tooling
children are excluded and unmeasured; independent session samples do not prove
a memory reduction. Report: `benchmarks/format-cursor-ollama-session.json`.

## Literal-safe bounded type-declaration presentation

Final snapshot: `target/declaration-layout-cow.exe`. Global type declarations,
local types and alias RHS declarations now share `hover/declaration`; expression
operator spacing is isolated in its child module. The printer reuses parsed
type nodes, field/member spans, expression identities and lexical tokens instead
of replacing text inside strings. It handles supported inline aggregate layout,
elastic field/tag columns, generic parameters/constraints, function/channel
syntax and precedence-based spacing. No Go/gofmt process is invoked by hover.

The old line-based attached-comment filter could delete a `//` line inside a
multiline raw struct tag. Literal spans now protect those lines. Attached field
comments keep their source layout; free inline comments are omitted in the
supported single-line path, as in the pinned AST printer. Struct-tag contents,
names and operand text are preserved. Alias RHS declarations use the same
presentation instead of appending unformatted source.

Resource limits reuse the query budgets: 16 KiB inline input, 256 tokens and
printing steps per annotation, 256 expression-walk steps and depth 32. Formatted
output is capped at 64 KiB before elastic padding is allocated. The line/comment
pass stops at 256 KiB; larger alias targets borrow the existing source through
`Cow` instead of allocating another copy. Existing multiline declarations,
unsupported expression forms (including slices/function literals), complex
multiline column groups and exhausted budgets keep source presentation. Full
go/format parity is unfinished; no source, AST or type cache survives a query.

The initial 12-case declaration fixture improves from 1/12 to 12/12 complete
hover responses. The expanded and audited fixture matches 29/29 distinct exact
identifier positions in
`benchmarks/type-declaration-fixture/audited-final.json`: aggregates, field/tag
alignment, nested types, aliases, grouped fields, function/channel types,
constraints, comments, array expressions and raw literals. Earlier expanded
reports included prefix-overlap duplicate positions for Tag/TaggedFields or
Method/Methods and are superseded for coverage. The audited report records this
correction; all intended cases remain present rather than being dropped.

The production/internal-test/external-test/importer fixture improves from 7/11
to 11/11, including all four formerly different Wrapper hovers and three token
responses. Fresh pinned gopls comparison:
`benchmarks/declaration-layout-cow/method-view.json`. The existing stdio e2e
checks field alignment, alias RHS presentation, and literal `//` lines in both
global and local raw tags. No new unit tests were added.

The final same-plan hover replays against unchanged pinned references remain
SDK 92/99 (`--limit 6`), Hugo 51/51, Tile38 36/36, Ollama 58/59, typescript-go
44/44 and Prometheus 50/50 (`--limit 3` for external corpora). No previously
matched pre-formatter row failed; seven SDK and one Ollama residuals remain.
Reports: `benchmarks/declaration-layout-cow/`, with fresh reference runs under
`benchmarks/declaration-layout/`. No new sampled hover request exceeded 100 ms;
this is not a controlled speed comparison.

Fmt check, full Rust tests, Clippy with warnings denied and release build pass.
The final Ollama 1068-file one-cycle session, with type shapes/definitions and
all 47 feature consistency checks, passes without failures. Root OS peak
working set is 58,269,696 bytes and sampled private bytes 48,746,496, both below
100,000,000. Driver and Go tooling children are excluded/unmeasured. Independent
samples do not prove a memory reduction. Report:
`benchmarks/declaration-layout-cow-ollama-session.json`.

## Literal-safe anonymous type and signature presentation

Anonymous type/signature hover previously applied aggregate spacing, array-length
and function-parameter rewrites inside struct tags. Multiline raw tags were split
as field lines, and grouped-field expansion collapsed spaces inside tags. The
shared normalization now uses existing lexer spans to preserve literal tokens,
skip literal delimiters when splitting fields/parameters and matching parentheses,
and retain the original grouped-field type/tag suffix. Comment removal and tag
decoding also reuse the lexer and shared string decoder. No dependency or retained
source/AST/type cache was added; the source-declaration printer remains separate
from semantic `types.TypeString` presentation.

The independent raw/interpreted tag fixture improves from 3/36 to 36/36 complete
hover responses against pinned gopls v0.24.0 / Go 1.27.3. It covers variable types,
function results and grouped fields with aggregate/function/array-looking text,
separators, unmatched literal braces/parentheses, repeated spaces, multiline raw
strings and escaped quotes. Reports and source/binary hashes:
`benchmarks/hover-tag-literals/expanded-baseline.json` and `fixed.json`.
The centralized stdio e2e checks literal preservation in grouped anonymous fields
and function results; no new unit tests were added.

Same-plan complete hover replays remain SDK 92/99, Hugo 51/51, Tile38 36/36,
Ollama 58/59, typescript-go 44/44 and Prometheus 50/50. No previously matched
row failed, and no sampled request exceeded 100 ms. The audited declaration
fixture remains 29/29 against its unchanged full reference; the production/test
view fixture remains 11/11 against a fresh reference (including three full token
responses). Reports: `benchmarks/hover-literal-safe/`; corpus reference chains
are retained in each report. These finite samples do not establish broad parity.

`cargo fmt --check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`
and release build pass for `target/hover-literal-safe.exe`.

The measured Ollama 1068-file open/edit/restore/close session passes the
100,000,000-byte gate with type shapes and type definitions enabled and no
consistency failures. Root OS peak working set is 58,703,872 bytes; sampled root
private bytes peak at 47,947,776. Driver and Go tooling children are excluded
and unmeasured. This independent sample does not establish a memory reduction.
Report: `benchmarks/hover-literal-safe-ollama-session.json`.

## Explicit generic function-value presentation

Generic-function hover now handles fully explicit instantiations used as values,
including package initializers and parenthesized expressions. It shares the
native `instantiated_signature` argument-count/constraint proof and the existing
signature-label renderer with call-site hover. Explicit-argument parsing is also
shared with call inference. Parameter/result names stay in query-local labels;
the compact structural signature nodes still carry only type identities.

Callable-value presentation follows explicit instantiations through local and
package variable initializers. Package variables use the existing local-binding
presentation over one query-local declaration wrapper, then requalify types for
the consuming file. This enables signature help through those variables and
preserves hover names/single-result presentation. The new package traversal caps
declarations at 16 KiB and depth at 32; labels reuse 16 KiB limits and explicit
type arguments remain capped at 32. No persistent source/AST/type cache or new
dependency was added. Generic hover code moved into `hover/generic.rs`, with the
package-initializer source-span lookup shared by the hover paths.

The authoritative focused comparison is
`benchmarks/generic-function-values/final.json`, compared with `final-baseline.json`
from the preceding literal-safe binary. All 20 complete hover responses now match
the pinned reference, versus 0/20 before. Cases include imported/local functions,
import aliases, parentheses, multiple type parameters, variadic and zero-input
functions, named results, constraints, local/global values and cross-package
consumers. All 11 explicit signature-help requests now return non-null signatures
with matching labels, parameters and active selections. Complete signature-help
responses still match 0/11: empty markdown documentation is omitted; the zero-input
case also emits an empty parameters list. These metadata differences remain open
and are retained as failures. No result was dropped or normalized to hide them.
Earlier experimental signature reports used missing/incomplete request context
and are superseded by these audited non-null, explicit-invocation samples.

Centralized stdio e2e verifies instantiated references, local/global variable
hover and signature-help labels. Incomplete/excess type arguments and a failing
`~int` constraint retain unknown rather than inventing function values. The
existing Go/types differential remains 1162 body matches, 1 unknown and no
incorrect/rejected/unverified results; declaration scope remains 1 match and
5 unknown. Report: `benchmarks/generic-function-values-type-oracle.json`.

Fmt check, full Rust tests, Clippy with warnings denied and release build pass
for `target/generic-function-values.exe`.

Same-plan hover replays remain SDK 92/99, Hugo 51/51, Tile38 36/36, Ollama 58/59,
typescript-go 44/44 and Prometheus 50/50, with no newly failed rows. The audited
type-declaration fixture remains 29/29, fresh package/test-view comparison 11/11,
and literal-safe tag comparison 36/36. Fresh full semantic-token responses match
SDK 6/6 and each external corpus 3/3 (21 distinct corpus requests total).
Reports: `benchmarks/generic-function-values-regression/`,
`benchmarks/generic-function-values-tokens/`,
`benchmarks/generic-function-values-tokens-sdk6.json`, and
`benchmarks/hover-tag-literals/generic-regression.json`.

The Ollama 1068-file one-cycle open/edit/restore/close session, with type
shapes/definitions and all 47 pinned feature consistency requests, has no
failures. Root OS peak working set is 58,138,624 bytes and sampled private bytes
peak at 47,845,376, below the 100,000,000-byte gate. Driver and Go tooling children
are excluded/unmeasured. This independent sample does not prove a reduction.
Report: `benchmarks/generic-function-values-ollama-session.json`.

## Bounded token-query inference reuse

Tracing `os/root_test.go` separated a repeated-work cost from parsing: one traced
baseline spent 68.8 ms collecting format calls, including 63.8 ms finding their
signature-derived format parameter and only 3.3 ms building scopes. Literal
element-key resolution separately spent about 31 ms on repeated table elements.
The existing `GOQUERY_TRACE_QUERIES` switch now reports aggregate format-call
candidate/scope/signature costs without adding another environment option.

The format collector keeps a per-active-scope memo for simple name/selector
callees, keyed by the resolved lexical binding selection and exact source text.
Both known arities and unknown outcomes are bounded by the existing query memo
limits (256 entries, 16 KiB key/value payload, plus bounded map metadata).
Shadowed bindings remain separate; evaluated receivers and other expressions
keep the uncached inference path. Dropping the active body also drops its memo;
no whole-file syntax or type graph is retained by this scan.

Source analysis interns identical elided-literal projections by parent context
and map-key/element direction, under the existing 4096-context/64-depth limits.
The resolver's existing 4096-entry literal-key memo now stores type-directed
field/expression facts for explicit and elided contexts. Map/array expression
keys still resolve their identities at every occurrence's own lexical position.
The temporary interning table is bounded by the same context budget; no cache
survives the query and no dependency was added.

Two settled fresh server runs per binary, alternated before the broader
verification workloads, reduce the SDK full-token request median from 269.2 ms
to 212.1 ms (about 21%). All four complete decoded responses match the retained
fresh pinned gopls reference. This finite request remains above the approximately
100 ms target; it does not establish broader latency acceptance. Evidence:
`benchmarks/sdk-token-phases/settled-ab.json` (full responses, hashes and reference
chain), with diagnostic phase reports alongside it. A traced optimized run
reduces the literal-element-key phase to 4.3 ms; trace timings are diagnostic,
not a separate controlled speed claim.

The expanded centralized stdio e2e checks repeated format calls across shadowed
bindings, returning to an outer binding, nested slice/struct literals, distinct
map key/value projections and expression keys. The separate focused comparison
retains 11/15 full matches with every native response unchanged from baseline:
`benchmarks/token-reuse-fixture/baseline.json` and `final.json`. Its four existing
residuals remain explicit: anonymous callable-field initializer keys are styled
as variables, generic-field highlights include instantiated uses absent from
gopls, and index/label highlights miss uses as elided map expression keys. No
case was removed to improve the score. The prior directive-cursor fixture remains
257/257 complete matches (`benchmarks/signature-format-fixture/token-reuse.json`).

Fmt check, full Rust tests, Clippy with warnings denied and release build pass
for `target/token-reuse.exe`.

Fresh six-corpus token/highlight comparisons retain all complete matches:
SDK tokens 6/6 and highlights 72/72; Hugo 3/3 and 36/36; Tile38 3/3 and 25/25;
Ollama 3/3 and 36/36; typescript-go 3/3 and 36/36; Prometheus 3/3 and 36/36.
This is 21 distinct full-token requests and 241 highlights, not counting the
smaller overlapping SDK checks. Reports:
`benchmarks/token-reuse-regression/`, `benchmarks/token-reuse-sdk-tokens.json`
and `benchmarks/token-reuse-sdk-highlights.json`. The concurrent SDK root-test
token sample took 372.7 ms; no other sampled request in these groups exceeded
100 ms. This concurrent run is separate from the settled A/B above.

The measured Ollama 1068-file one-cycle open/edit/restore/close workload, with
type shapes/definitions and all 47 pinned feature consistency requests, has no
failures. Root OS peak working set is 58,118,144 bytes and sampled private bytes
peak at 47,181,824, below the 100,000,000-byte gate. Driver and Go tooling children
are excluded/unmeasured; this independent sample is not a memory-reduction
comparison. Report: `benchmarks/token-reuse-ollama-session.json`.

## Literal expression keys and guard declaration identities

Document highlight's cheap role filter incorrectly rejected every literal key
for local and non-method global cursors, including keys of elided map/array
literals that are ordinary expressions. Literal keys now reach the existing
type-directed resolver. Type-switch clause matches also require a resolved local
identity before checking the clause scope, so a same-spelling struct field label
does not become a use of the implicit clause variable.

The type-switch guard declaration itself has no go/types object. Highlight now
keeps it in the pinned gopls nil-object group, independently of the distinct
implicit variables declared in each case. Multiple same-spelling guard
declarations match one another without including the case-body variables. This
uses existing switch-binding metadata; no inference or spelling guess was added.

Source analysis also marks anonymous struct initializer keys as literal field
labels using the parsed type expression and `struct` token. Anonymous callable
field keys now receive property tokens. A same-spelling package variable's
highlight/rename no longer includes those labels. Map/array type syntax retains
ordinary key-expression name roles. No dependency, retained cache or new budget
was introduced; existing bounded analysis and resolution paths are reused.

The dedicated local/global/constant/shadowed/key/field/type-switch fixture improves
from 11/40 to 40/40 complete highlight responses against pinned gopls v0.24.0 / Go
1.27.3. Reports: `benchmarks/literal-key-highlights/expanded-baseline.json` and
`final.json`. Its plans cover every occurrence of the four selected identifiers,
including both guard declarations and all case/field/key uses, without prefix
overlap positions. The expanded mixed token/highlight/definition/rename fixture
improves from 12/19 to 17/19 complete responses:
`benchmarks/token-reuse-fixture/expanded-key-baseline.json` and
`literal-key-final.json`. Remaining full-response failures are the generic-field
highlight identity difference and anonymous field rename missing its initializer
key and selector uses. They remain recorded rather than excluded.

Centralized stdio e2e checks complete highlight ranges for parameter keys,
same-spelling struct fields, shadowing, type-switch clauses and guard declarations;
it also checks anonymous callable-key token style and the complete edit set for
renaming a package variable independently of a same-spelling anonymous field.
No new unit tests were added. Fmt check, full Rust tests, Clippy with warnings
denied and release build pass for `target/literal-key-identities.exe`.

Fresh complete-response corpus comparisons remain SDK tokens 6/6 and highlights
72/72; Hugo 3/3 and 36/36; Tile38 3/3 and 25/25; Ollama 3/3 and 36/36;
typescript-go 3/3 and 36/36; Prometheus 3/3 and 36/36. That is 21 distinct
full-token and 241 highlight requests, excluding overlapping smaller SDK checks.
Reports: `benchmarks/literal-key-regression/`,
`benchmarks/literal-key-sdk-highlights.json` and
`benchmarks/literal-key-sdk-tokens.json`. The SDK root-test token request remains
slow at 215.9 ms; no other sampled request in these groups exceeds 100 ms.

The same-plan hover replays retain SDK 92/99, Hugo 51/51, Tile38 36/36,
Ollama 58/59, typescript-go 44/44 and Prometheus 50/50 without newly failed rows;
the audited type-declaration fixture remains 29/29. Reports:
`benchmarks/literal-key-hover-regression/` (retained complete reference chains).

The Ollama 1068-file one-cycle open/edit/restore/close workload, with type
shapes/definitions and all 47 pinned feature consistency requests, passes without
failures. Root OS peak working set is 58,507,264 bytes and sampled private bytes
peak at 49,082,368, below the 100,000,000-byte gate. Driver and Go tooling children
are excluded/unmeasured; independent samples do not establish a memory change.
Report: `benchmarks/literal-key-ollama-session.json`.

## Anonymous field rename edit expansion

The explicit `AnonymousField` edit branch previously returned only the field's
declaration. It now uses a reference collector in the existing query/reference
layer, sharing anonymous-field declaration resolution with navigation. Candidate
files use the existing dependency/package/reference scope and identifier-domain
filter. Each candidate is analyzed once for the selected name and keeps only
bounded coordinate rows; the existing 4096-reference limit rejects overflow.
Analysis/read failures abort this collection instead of returning its prefix.

Resolution preserves the written type's file and byte origin through local
copies, package values, imported selectors, function results and container
projections. A prepared occurrence reuses source analysis's locals rather than
reparsing its enclosing body. Expression-member spans handle call/index
receivers. Inline indexing now includes nested struct declarations, and a key
must actually be followed by `:` before using literal-key lookup. Elided literal
contexts expose their existing bounded projection path so declaration queries
do not lose provenance by rendering/reparsing a type string. Map keys and
different same-shaped anonymous type expressions remain separate identities.

Rename also rejects a direct field-name conflict in the same anonymous owner
and applies the existing cross-package unexport check. Protocol position/error
formatting shares the existing helper, now accepting an already-read source.
Declaration text is dropped before scanning reference candidates. Existing
16 KiB type/projection input, depth/projection and reference budgets remain;
there is no new dependency or persistent source/AST/type cache.

The corrected, offline-compiled 24-request fixture improves from 1/24 to 24/24
complete responses against pinned gopls v0.24.0 / Go 1.27.3. It covers global,
local/copy, nested/slice fields, same-package/cross-package consumers, returned
anonymous types, mixed map/struct keys, distinct declarations, two direct
conflicts and unexport refusal. Reports:
`benchmarks/anonymous-field-rename/expanded-baseline.json` and `final.json`, with
source/binary hashes and full plans. The original experimental baseline had an
invalid nested-field type elision and is superseded; the corrected fixture passes
offline `go test -run '^$' ./...` (compile preparation, not a corpus build).

The earlier expanded mixed fixture improves from 17/19 to 18/19, including the
previously incomplete anonymous callable-field rename:
`benchmarks/token-reuse-fixture/anonymous-rename-final.json`. Its remaining
generic-field highlight identity difference stays recorded. Centralized stdio
e2e checks the full anonymous-field edit sets across two files, copy propagation,
distinct declaration exclusion and conflict rejection. No new unit tests were
added. Fmt check, full Rust tests, Clippy with warnings denied and release build
pass for `target/anonymous-field-rename.exe`.

Fresh corpus prepare/rename comparisons retain SDK 29/30, Hugo 15/15, Tile38
57/57, Ollama 19/19, typescript-go 51/51 and Prometheus 55/55 complete matches.
The SDK `runtime/mgcpacer.go:init@method.rename` residual is also present in the
preceding binary's same-plan replay, with no newly failed row. The first
`--limit 1` plans were empty for SDK/Tile38/typescript-go/Prometheus and are not
acceptance evidence; the nonempty SDK/Tile38/typescript-go/Prometheus runs use
`--limit 6`. Reports: `benchmarks/anonymous-rename-corpus/`,
`benchmarks/anonymous-rename-{sdk,tile38,typescript,prometheus}-rename.json` and
`benchmarks/anonymous-rename-sdk-baseline.json`.

Fresh tokens/highlights remain SDK 6/6 and 72/72; Hugo 3/3 and 36/36; Tile38 3/3
and 25/25; Ollama 3/3 and 36/36; typescript-go 3/3 and 36/36; Prometheus 3/3 and
36/36. This is 21 distinct full-token and 241 highlight requests, excluding
smaller overlapping SDK checks. Reports: `benchmarks/anonymous-rename-regression/`
and `benchmarks/anonymous-rename-sdk-{tokens,highlights}.json`. Hover replays remain
SDK 92/99, Hugo 51/51, Tile38 36/36, Ollama 58/59, typescript-go 44/44 and
Prometheus 50/50, with no newly failed row, under
`benchmarks/anonymous-rename-hover/`; the audited declaration fixture remains
29/29. These finite comparisons do not prove overall semantic parity.

The Ollama 1068-file one-cycle open/edit/restore/close workload, with type
shapes/definitions and all 47 pinned feature consistency requests, passes with
no failures. Root OS peak working set is 58,232,832 bytes and sampled private
bytes peak at 48,508,928, below the 100,000,000-byte gate. Driver and Go tooling
children are excluded/unmeasured. Independent samples do not establish a memory
reduction. Report: `benchmarks/anonymous-rename-ollama-session.json`.

## Anonymous-field references

References and rename now share anonymous-field target resolution and the
bounded provenance collector. Declaration filtering happens before collecting
locations. The focused fixture matches 46/46 complete responses against pinned
gopls, including both includeDeclaration settings, unused fields and an unused
function (`benchmarks/anonymous-field-rename/references-commit-check.json`).
Resolved empty reference lists return `[]`, while unavailable targets retain
`null`; five protocol goldens intentionally update 17 empty results. Centralized
stdio e2e covers declaration filtering and exact anonymous-field reference sets.
Fmt check, full Rust tests and Clippy with warnings denied pass. These focused
comparisons do not establish whole-corpus reference parity or a memory reduction.

## Remaining work

- Offline module-graph enumeration now succeeds. Verify matching package scope
  beyond the current bounded Hugo feature matrix with fixed dependencies.
- Expand the Hugo implementation/highlight coverage, including universe-only
  and instantiated generic interfaces, and reduce the remaining slow
  implementation queries. Preserve exact identities and request-local budgets.
- Investigate the expanded SDK view-admission/budget residuals and Ollama's
  remaining dependency-error hover. Keep all full responses.
- Broaden format/directive highlight coverage beyond the now-matched finite
  fixture while preserving parser/association budgets.
- Broaden anonymous-field rename coverage beyond the now-matched finite
  provenance fixture; investigate generic-field highlight identity against the
  pinned reference. The expanded token-reuse fixture retains that difference.
- Complete declaration-presentation coverage beyond the supported inline path;
  preserve complex multiline layouts and literals while extending it.
- Investigate native arena test-view identity and general constant array-length
  expressions, preserving bounded inference.
- Expand native generic-call hover coverage beyond the now-matched focused
  fixtures, including contextual/partial function-value instantiation and
  unsupported type projections. Complete signature-help documentation and
  empty-parameter wire parity.
- Extend syntax/type/LSP differential coverage across all existing projects;
  these small samples do not establish overall parity.
- Run the measured Ollama memory gate for any changes affecting allocations or
  retained state. Preserve bounded request-local parsing and existing layering.

Validation performed for the first Rust fix: `cargo fmt --check`, `cargo test`,
`cargo clippy --all-targets -- -D warnings`, and `cargo build --release`.
