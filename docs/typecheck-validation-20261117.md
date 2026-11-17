# Native semantic validation — 2026-11-17

The native semantic migration is implemented. Semantic queries share one
request-local arena, declaration identities and checked regions. Completion,
hover, navigation, references and structural relations use those facts rather
than independent type trees. Language versions and build views come from
workspace-owned metadata. Missing facts and exhausted budgets remain unknown.

This report distinguishes implemented architecture, measured behavior and
remaining compatibility gaps. It does not claim complete Go compiler or gopls
equivalence.

## Fixed inputs

All semantic comparisons use Go 1.27.3 and gopls v0.24.0 compiled with Go 1.27.3.
The standard library is the primary corpus. External revisions are:

| Project | Revision |
| --- | --- |
| Tile38 | `a9f953e358d31bc8fc9479ad71f132b1ed752b4b` |
| Ollama | `5e983d4f800053c6f5691496b675c3212aa8099d` |
| typescript-go | `89d5d5b2849a0db0957065889ca58536fa6d2e4a` |
| Prometheus | `0b1983060510028164ab063444a0b64c0ca4b69d` |
| Hugo | `ef7c52509321b14b0dfce34ef04be010b37ca05d` |

Both servers receive the same workspace roots, open buffers and capabilities.
The SDK's `cmd` module is selected explicitly for compiler completion queries;
opening a compiler file from a `std` workspace instead admits a bounded orphan
package and does not establish equivalent module scope.

Complete responses are retained. Reference and edit ordering is normalized
where order is unspecified; semantic tokens are compared after decoding each
negotiated legend, preserving categories and modifiers. Formatting additionally
compares the resulting source. Raw response differences remain distinct from
these semantic comparisons. Compiler/syntax diagnostics are informational;
analysis hints are excluded from their compatibility totals.

## Dependency preparation and compilation

The final Linux run uses a separate, complete offline module cache containing
1,260 archives, 213,838 source files and 4,638,708,307 uncompressed bytes.
Earlier measurements used a cache containing empty directory stubs; those
measurements are retained as preparation failures and are superseded here.
No dependency download occurs in the server.

The following checks use Linux/amd64, `CGO_ENABLED=0`, `GOWORK=off`,
`GOTOOLCHAIN=local` and offline dependency resolution:

| Corpus | Packages loaded | Packages with loading errors | Full build |
| --- | ---: | ---: | --- |
| Standard library | 359 | 0 | Passed |
| Tile38 | 23 | 0 | Passed |
| Ollama | 96 | 0 | Failed: MLX `Array` definitions unavailable |
| typescript-go | 105 | 0 | Passed |
| Prometheus | 114 | 0 | Passed |
| Hugo | 192 | 0 | Passed |

Successful package loading is not a full build. Ollama's full build remains
unavailable in this configuration; its LSP and memory workloads are separate
evidence.

## Native checker contracts

Windows (Rust 1.96.0) and Linux (Rust 1.99.1) are checked with formatting,
Clippy across all targets with warnings denied, and real stdio end-to-end tests.
The stdio cases exercise complete replies, unsaved edits, build views, language
versions, canonical member identities and bounded method searches.

At the body-checker checkpoint, the pinned go/types comparison matched
1,162/1,163 function-body bindings, with one unknown (`largeComplex`) and zero
incorrect inferred types. Six declaration-only parameters are reported
separately as unknown and do not count as body coverage. This fixture does not
prove complete go/types parity.

## Complete-response comparisons

Matched/total responses, including reported errors. The final implementation
search change was rechecked on every corpus; signature and hierarchy checks
were also repeated on that build. Other rows use the preceding build with the
same semantic body checker. This is targeted regression evidence, rather than
a claim that every recorded response was produced by one binary.

| Feature | Standard library | Tile38 | Ollama | typescript-go | Prometheus | Hugo |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Hover | 347/351 | 343/345 | 293/348 | 312/312 | 343/344 | 334/344 |
| Signature help | 40/40 | 40/40 | 43/43 | 37/37 | 48/48 | 48/48 |
| Rename | 59/59 | 131/133 | 103/106 | 93/93 | 128/128 | 106/106 |
| Implementation | 89/89 | 110/110 | 131/131 | 47/47 | 157/157 | 124/132 |
| Document highlight | 120/120 | 109/109 | 120/120 | 118/118 | 119/120 | 117/120 |
| Document symbols | 15/15 | 15/15 | 15/15 | 15/15 | 15/15 | 15/15 |
| Document links | 15/15 | 15/15 | 15/15 | 15/15 | 15/15 | 15/15 |
| Folding ranges | 10/10 | 10/10 | 10/10 | 10/10 | 10/10 | 10/10 |
| Code lenses | 12/12 | 12/12 | 12/12 | 12/12 | 12/12 | 12/12 |
| Formatting | 15/15 | 15/15 | 15/15 | 15/15 | 15/15 | 15/15 |
| Compiler/syntax diagnostics | 149/149 | 30/30 | 14/228 | 11/11 | 35/35 | 17/17 |
| Code actions | 16/16 | 16/16 | 16/16 | 16/16 | 16/16 | 16/16 |
| Definition | 18/18 | 18/18 | 18/18 | 18/18 | 20/20 | 18/18 |
| Declaration | 77/77 | 100/100 | 88/89 | 84/84 | 106/106 | 74/75 |
| Semantic tokens | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 |
| Semantic token ranges | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 |

The combined result is 5,698/6,003. Excluding informational diagnostic
notifications, it is 5,442/5,533. Strict raw response equality is 5,006/5,533
before the ordering, edit-decomposition and legend normalization described
above. No feature/corpus pair crashed or lacked a plan in the final complete
matrix.

Supplemental full-response results retain their differences:

| Workload | Matched/total |
| --- | ---: |
| SDK mixed queries, plain / snippets | 20/24; 19/24 |
| SDK type completion | 0/2 |
| Compiler member completion | 0/7 |
| Type definitions, core / SDK / compiler | 34/34; 14/14; 3/3 |
| References, standard library / external corpora | 16/18; 32/32 |
| Signature-help fixture | 39/39 |
| Signature-call fixture | 11/12 |
| Completion types / members / generic snippets | 31/33; 9/14; 11/12 |
| Generic members / unimported generic results | 25/28; 5/6 |
| Elided literals | 7/7 |
| Interface / struct / generic hierarchy versus gopls | 49/76; 82/83; 85/89 |
| Ollama / Tile38 mixed replays | 43/47; 16/21 |

All seven compiler completion sites contain the expected direct labels on both
servers, but none has an identical complete reply; label presence is not counted
as response parity. External reference replies match Tile38 8/8, Ollama 6/6,
typescript-go 6/6, Prometheus 6/6 and Hugo 6/6. The independent hierarchy
go/types oracle checks 74 complete semantic replies and two explicit
invalid-editor policies successfully. Its
int/string instantiation basis applies only to this fixture. All 46 repeated
hierarchy queries preserve their complete public replies and meet the 100 ms
limit in that fixture.

## Latency

The external-corpus 100 ms target is not fully met. There are 58 over-limit
external request samples in the final matrix, references and mixed replays,
plus 44 standard-library samples, including repeated SDK queries. Every sample
is listed in
[the latency table](typecheck-latency-20261117.csv); initialization is excluded.
Rename samples remain visible but have lower priority.

The largest external samples are seven Prometheus parser implementation
requests at 1,970–1,978 ms, Hugo's `Name` implementation query at 420 ms, and an
Ollama model-reference replay at 371 ms. These are observed LSP wall times,
including queued work and transport, rather than isolated algorithm timings.

The initial checkpoint matched only 914 bindings because local formal budgets
leaked across function regions. Every transition now rewinds local facts;
bounded global declarations are reconstructed below the next body's permanent
watermark. Real stdio coverage also checks generic navigation, cross-package
members, build views, language-version updates, completion edits and signature
documentation identities. The final stdio suite contains 18 passing tests.

## Remaining capability limits

Full compiler diagnostics, arbitrary-precision constant forms and every generic
constraint inference path are not implemented. Generated declarations or orphan
siblings outside documented budgets may remain unknown. Deep completion,
presentation ordering and generic instantiated-object behavior can differ from
gopls. These are compatibility gaps, not evidence of full parity.

The four standard-library hover residuals concern a generated syscall constant,
an omitted documentation link for `bytes.Count`, an invalid-type editor
expression and the large runtime tracing aggregate. Tile38 also retains a
package-variable hover and `any` versus `interface{}` presentation difference.
Hugo retains eight implementation-set differences, including error wrappers,
HTML string types and generic version relations. Five prepare-rename replies
remain unknown for Tile38 AMQP fields and Ollama MLX fields. Generic
instantiated-object highlighting and two declaration targets also differ.
The Ollama diagnostic union includes unavailable MLX definitions and diagnostics
for unopened files; its 14/228 result is not treated as diagnostic parity.

The finite whole-project memory workload does not prove a universal bound on
memory added by every individual request.

## Windows root-process memory

The final binary (`413b8c1a0f7de391e2d5aaba777417849244edc3a685116a1470ad522dd7c93f`)
passed three full open/edit/restore/close cycles over Ollama's 1,068 Go files
(12,073,161 source bytes), including semantic, signature and type-navigation
probes. Complete responses remained stable after restoration.

| Process | OS peak working set, bytes | Sampled private bytes |
| --- | ---: | ---: |
| Rust server root | 59,854,848 | 50,184,192 |
| `go env` child, supplemental sample | 10,285,056 | 47,099,904 |
| `gofmt` child, supplemental sample | 5,038,080 | 11,157,504 |

Both root metrics pass the strict 100,000,000-byte limit. Children and the client
are excluded from the root values. Child figures come from a separate
initialization/formatting session; very short-lived peaks may escape sampling,
so they are observed maxima rather than whole-workload child peak guarantees.

## Linux supplemental memory and build identity

The same three-cycle Ollama workload passed with 18,142 samples and 15,456
request barriers. Peak RSS/VmHWM was 53,342,208 bytes; sampled private
clean-plus-dirty memory was 51,355,648 bytes. These procfs values describe only
the Rust root process and are separate from the Windows working-set/private
commit gate. They are not interchangeable metrics.

| Build | SHA-256 |
| --- | --- |
| Linux complete-matrix checkpoint | `809dd4710167d88beea8c5b534ced7645a7c9f95786f4526bcb32f8756222769` |
| Linux final implementation, hierarchy and memory checks | `aadcdafb61aa442fec27fe627c59142030c27c47bf977aec2502a6a89e4c18f8` |
| Pinned gopls executable | `88699f0f9b9612a87a3fefc7518fe150aafc147a223ac54f6030596947acc83a` |

Final Rust sources were verified byte-for-byte between the local workspace and
the Linux validation copy. Formatting, all-target Clippy and 18 stdio end-to-end
tests pass on both hosts. Remaining differences and over-limit latencies above
remain open; this validation is not an all-feature acceptance-pass declaration.
