# Semantic latency checkpoint

Go 1.27.3 / gopls v0.24.0; release snapshot `target/embedded-universe-final.exe`.
These feature groups ran concurrently with an Ollama memory session. Values are
observed request times, not isolated throughput or a controlled performance A/B.
All requests above 100 ms are listed; reports retain complete responses.

| Feature | Corpus | Request | ms |
| --- | --- | --- | ---: |
| implementation | hugo | `hugolib/page__common.go:pageCommon@1241` | 175.4 |
| implementation | hugo | `hugolib/page__common.go:getNextPrev@1017` | 185.8 |
| implementation | hugo | `hugolib/page__common.go:getNextPrevInSection@1169` | 191.7 |
| implementation | hugo | `hugolib/page__common.go:Store@2758` | 182.7 |
| implementation | hugo | `hugolib/page__common.go:Scratch@2846` | 203.2 |
| implementation | hugo | `hugolib/page__common.go:pageCommon@1005` | 187.6 |
| implementation | hugo | `hugolib/page__common.go:pageCommon@1157` | 189.8 |
| implementation | prometheus | `promql/info.go:infoSeriesReference@4076` | 169.9 |
| implementation | prometheus | `promql/info.go:evalInfo@1302` | 208.4 |
| implementation | prometheus | `promql/info.go:equal@4180` | 154.0 |
| implementation | prometheus | `promql/info.go:infoSelectHints@7301` | 123.3 |
| implementation | prometheus | `storage/remote/codec_test.go:mockChunkSeriesSet@28362` | 119.1 |
| implementation | stdlib | `internal/coverage/test/counter_test.go:ctrVis@329` | 341.9 |
| implementation | stdlib | `internal/coverage/test/counter_test.go:VisitFuncs@400` | 162.7 |
| implementation | stdlib | `internal/coverage/test/counter_test.go:ctrVis@392` | 232.1 |
| implementation | stdlib | `internal/coverage/test/counter_test.go:ctrVis@1229` | 168.1 |
| implementation | stdlib | `crypto/internal/fips140/aes/gcm/gcm_nonces.go:GCMWithCounterNonce@3184` | 359.5 |
| implementation | stdlib | `crypto/internal/fips140/aes/gcm/gcm_nonces.go:GCMWithXORCounterNonce@7489` | 158.7 |
| implementation | stdlib | `crypto/internal/fips140/aes/gcm/gcm_nonces.go:NonceSize@3359` | 242.6 |
| implementation | stdlib | `crypto/internal/fips140/aes/gcm/gcm_nonces.go:Overhead@3438` | 135.4 |
| implementation | stdlib | `crypto/internal/fips140/aes/gcm/gcm_nonces.go:Seal@3684` | 130.7 |
| implementation | stdlib | `crypto/internal/fips140/aes/gcm/gcm_nonces.go:Open@4696` | 142.7 |
| implementation | stdlib | `crypto/internal/fips140/aes/gcm/gcm_nonces.go:GCMWithCounterNonce@2090` | 148.5 |
| implementation | stdlib | `crypto/internal/fips140/aes/gcm/gcm_nonces.go:GCMWithCounterNonce@2235` | 218.5 |
| implementation | stdlib | `crypto/internal/fips140/aes/gcm/gcm_nonces.go:GCMWithXORCounterNonce@5531` | 126.5 |
| implementation | stdlib | `crypto/internal/fips140/aes/gcm/gcm_nonces.go:GCMWithXORCounterNonce@5679` | 143.7 |
| implementation | stdlib | `math/rand/normal.go:NormFloat64@835` | 238.7 |
| rename | hugo | `resources/images/config.go:Actions@rename` | 111.3 |
| rename | hugo | `resources/images/config.go:TargetFormat@field.rename` | 193.8 |

Reports: `benchmarks/embedded-universe-final/*-*.json`.

Implementation latency remains unfinished; matching results do not satisfy the
approximately 100 ms target for these requests. Rename is lower priority.

## Isolated Hugo implementation rerun

After the other workloads completed, the same fixed release matched 17/17.
Report: `benchmarks/embedded-universe-final-hugo-implementation-isolated.json`.
The following requests in `hugolib/page__common.go` still exceed 100 ms:

| Request | ms |
| --- | ---: |
| `pageCommon@1241` | 185.5 |
| `getNextPrev@1017` | 190.4 |
| `getNextPrevInSection@1169` | 192.2 |
| `Store@2758` | 189.3 |
| `Scratch@2846` | 188.4 |
| `pageCommon@1005` | 184.5 |
| `pageCommon@1157` | 182.1 |

These are not explained solely by concurrent corpus preparation. The maximum
was 192.2 ms; this remains unfinished performance work.

The existing `GOQUERY_TRACE_QUERIES` phase timers were replayed over the same
17 requests (`benchmarks/embedded-universe-hugo-trace.log`). One `pageCommon`
supertype search took 187.2 ms: matching 163.4 ms, building 134 method sets
13.3 ms and collecting candidates 0.6 ms, with 186 cached signatures. The timed
scope does not distinguish source I/O from signature canonicalization, so a
deeper profile is still needed. The next performance investigation should focus
on repeated signature proofs, rather than assuming the workspace walk dominates.

## Nongeneric declaring-owner optimization

Implementation signature construction now skips rebuilding the promotion graph
when the selected declaring owner has no type parameters. Lookup has already
selected the declaration; its signature still uses the declaration's own lexical
origin. Generic declaring owners retain the substitution/environment walk.
There are no new caches, dependencies or raised budgets.

An isolated A/B replay used the same 17 complete Hugo requests and fresh servers
in baseline/candidate/candidate/baseline order. Both binaries are fixed snapshots;
hashes, responses and individual times are retained in
`benchmarks/signature-owner-hugo-ab.json`. All 17 responses were unchanged across
all four runs. The following values are medians of two replays per binary, not
tail-latency guarantees:

This initial replay sends queries immediately after opening files; it does not
wait for diagnostic notifications to become quiet. Those request times may
include queued server work and are distinct from the settled feature harness.

| Request in hugolib/page__common.go | Baseline ms | Candidate ms |
| --- | ---: | ---: |
| `pageCommon@1241` | 184.2 | 88.6 |
| `getNextPrev@1017` | 181.4 | 95.2 |
| `getNextPrevInSection@1169` | 190.1 | 87.1 |
| `Store@2758` | 185.9 | 91.2 |
| `Scratch@2846` | 182.4 | 90.1 |
| `pageCommon@1005` | 179.3 | 87.3 |
| `pageCommon@1157` | 179.9 | 93.3 |

`nextPrevInSectionProvider@1073` still varied: median 138.2 ms baseline versus
131.0 ms candidate. These samples include post-open queued work; the settled
comparison below separates this observation from steady query execution. Keep
the full slow-request evidence rather than inferring a global latency guarantee.

### Settled A/B confirmation

After all other workloads completed, both binaries were replayed with the
harness's existing notification quiet wait enabled before queries. Order was
candidate/baseline/baseline/candidate, with the same buffers and capabilities.
All 17 complete responses remained identical across all four runs. Report:
`benchmarks/signature-owner-hugo-ab-settled.json`.

| Request in hugolib/page__common.go | Baseline median ms | Candidate median ms |
| --- | ---: | ---: |
| `pageCommon@1241` | 184.7 | 90.9 |
| `getNextPrev@1017` | 184.2 | 89.2 |
| `getNextPrevInSection@1169` | 188.2 | 87.7 |
| `Store@2758` | 188.0 | 88.8 |
| `Scratch@2846` | 190.0 | 83.6 |
| `pageCommon@1005` | 184.7 | 88.6 |
| `pageCommon@1157` | 187.4 | 88.9 |

Every candidate request in these two settled replays was below 100 ms; the
maximum was 94.4 ms. `nextPrevInSectionProvider@1073` measured 63.3 ms baseline
and 61.7 ms candidate (medians). This supports a settled-query speedup for this
finite plan, while the separate post-open samples retain startup scheduling
costs. Broader corpus and tail-latency acceptance remain unfinished.

The final six-corpus implementation comparisons under `benchmarks/signature-owner/`
matched 103/103 complete responses. No sampled external request exceeded 100 ms;
the SDK request `crypto/internal/fips140/aes/gcm/gcm_nonces.go:Overhead@3438`
took 146.3 ms in the concurrent comparison and remains listed as a slow request.

## Native generic-hover checkpoint

Snapshot `target/native-generic-final.exe`; reports under
`benchmarks/native-generic/`. SDK, external hover/token groups, Hugo relations
and the Ollama memory workload overlapped. The following observed requests
exceeded 100 ms; these times do not establish a regression or speedup against
the settled A/B above. Complete responses remain in the reports.

| Feature | Corpus | Request | ms |
| --- | --- | --- | ---: |
| implementation | hugo | `hugolib/page__common.go:nextPrevProvider@939` | 123.2 |
| implementation | hugo | `hugolib/page__common.go:nextPrevInSectionProvider@1073` | 129.7 |
| implementation | hugo | `hugolib/page__common.go:pageCommon@1241` | 296.6 |
| implementation | hugo | `hugolib/page__common.go:getNextPrevInSection@1112` | 118.5 |
| implementation | hugo | `hugolib/page__common.go:getNextPrev@1017` | 452.0 |
| implementation | hugo | `hugolib/page__common.go:getNextPrevInSection@1169` | 184.5 |
| implementation | hugo | `hugolib/page__common.go:Store@2758` | 137.2 |
| implementation | hugo | `hugolib/page__common.go:Scratch@2846` | 108.6 |
| implementation | hugo | `hugolib/page__common.go:pageCommon@1157` | 108.1 |
| semantic tokens | stdlib | `os/root_test.go` | 229.6 |

## Signature-based format checkpoint

Final replay `target/signature-format-unicode.exe` against the same-plan cached
pinned gopls responses, under `benchmarks/signature-format-unicode/`. The replay
overlapped the Ollama memory session. The SDK semantic-token request for
`os/root_test.go` took 267.9 ms; it is the only request above 100 ms in these
SDK/external hover, token and document-highlight groups. The preceding fresh
reference matrix (`target/signature-format-bounded.exe`) observed 252.0 ms for
the same request. Neither run is a controlled latency A/B, and this request
still misses the approximately 100 ms target.

## Scoped format-cursor query A/B

`benchmarks/format-cursor/sdk-scope-ab.json` compares the functional whole-file
collector (`target/format-cursor-final.exe`) and scoped collector
(`target/format-cursor-scoped.exe`). Two fresh settled server runs per binary
were alternated after the other task workloads finished. All complete responses
match the fresh pinned gopls reference retained in `sdk-directives.json`.

| os/root_test.go request | Baseline median ms | Scoped median ms |
| --- | ---: | ---: |
| `root.Open directive@8828` | 69.2 | 4.7 |
| `root.Open directive@9505` | 61.3 | 3.5 |

These measurements support faster association queries for these two cursors.
Token generation still visits all format literals and is not covered by this
speedup. No >100 ms request occurred in the associated six-corpus highlight
replays or focused 257-query fixture, but broader latency acceptance remains
unfinished.

## Type-declaration presentation checkpoint

`target/declaration-layout-cow.exe` passed the focused audited 29-case type
declaration comparison, the 11-case test-view comparison and the SDK/external
hover replays under `benchmarks/declaration-layout-cow/`. No replayed corpus
hover or test-view request exceeded 100 ms. These checks overlapped other work;
they do not establish a controlled speedup or general latency acceptance. The
previously listed slow semantic-token and implementation requests remain open.
