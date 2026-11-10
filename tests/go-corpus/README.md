# Go source corpus

The corpus is the official `golang/go` GitHub archive at tag `go1.27.3`.
Sources stay outside the repository in the system temporary directory.

Prepare the pinned `golang/go` source archive outside the repository, then run:

```powershell
cargo test --test stdio go_source_package_corpus -- --ignored --nocapture
```

The default root is `$env:TEMP/goquery-go1.27.3/go-go1.27.3`. Set
`GOQUERY_GO_SOURCE` to use another checkout with the same VERSION. The external
gate requires Go 1.27.3 for its oracle. Normal tests remain fast and do not fetch
or require this external source tree.

`skip-packages.json` lists exact relative `src/` directories and reasons. There
are no wildcard skips; new directories become enabled automatically, stale entries
and empty reasons fail. Initial pending entries are a migration inventory, not
verified parser failures. Remove entries only after running the package gate.

To investigate a skipped package without modifying the baseline:

```powershell
$env:GOQUERY_GO_PACKAGE = 'internal/stringslite'
cargo test --test stdio go_source_package_corpus -- --ignored --nocapture
Remove-Item Env:GOQUERY_GO_PACKAGE
```

To rescan the entire standard-library scope regardless of the skip inventory:

```powershell
$env:GOQUERY_GO_SCOPE = 'stdlib'
cargo test --test stdio go_source_package_corpus -- --ignored --nocapture
Remove-Item Env:GOQUERY_GO_SCOPE
```

To include the compiler and other toolchain packages as well:

```powershell
$env:GOQUERY_GO_SCOPE = 'toolchain'
cargo test --test stdio go_source_package_corpus -- --ignored --nocapture
Remove-Item Env:GOQUERY_GO_SCOPE
```

The `toolchain` scope checks every directory outside `testdata`, bypassing the
skip inventory. The `stdlib` scope includes all non-cmd directories outside
`testdata`, including vendored
dependencies, internal packages, tests and platform variants. No build tags are
used to select files. Package overrides take precedence over this scope.

The runner discovers every `.go` file in each enabled directory, including tests
and platform-specific files. Build tags are not used to hide syntax from this
gate. The official parser must accept each file and independently enumerates
function and function-literal body ranges plus all function/method headers,
including anonymous functions and bodyless declarations. It also enumerates type
annotations in type/value declarations, fields, signatures and type assertions.
Interface method signatures are tested as function types by supplying the absent
`func` keyword; unions and approximations use their enclosing constraint grammar.
Generic type-parameter lists are independently parsed as synthetic function
headers, including local type declarations. Every package-level variable/constant
initializer expression is also enumerated independently by the oracle.
Each complete file is now validated by the handwritten `validate_file` entry
point, including package/import ordering, top-level declarations, headers and
nested bodies. Each header, type, initializer and body is also checked independently.
The complete token stream is compared with `go/scanner`
for token kind, UTF-8 byte spans and inserted semicolons, including file-level
declarations and signatures. Processing is package-by-package and file-by-file; AST
shapes and pretty-printed parser output are not compared.

This is a **full-file and component acceptance gate** for the pinned positive
source corpus, not proof of every Go language rule or semantics. The focused
positive/negative file corpus is in `tests/fixtures/frontend/files.json`; it
compares acceptance and validates diagnostic spans, not AST output. Systematic
negative testdata classification remains incomplete. Identifier boundaries use
Go's pinned Unicode 17.0.0 Letter/Decimal Digit categories; centralized tests
check every scalar value through the public lexer, plus parser acceptance and
scanner token spans for selected boundary files. Numeric scanning now validates
radix digits, required mantissas and exponents, and digit separators; boundary/mutation tests compare expression and
file acceptance in declarations, array lengths and nested bodies. Numeric value
legality/type checking remains semantic work. Many `testdata` directories contain intentional
syntax errors; their negative-case classifications remain pending. The eventual
goal is to remove all pending skips with appropriate positive/negative checks.

The verified default scope now covers **800 directories, 6,821 files, 110,203
headers, 394,492 type contexts, 179,374 package-level initializers and 104,246
function bodies**, including the compiler, assembler, linker and vendored toolchain
dependencies. All files receive full-file validation, independent component checks
and scanner token/span comparison. No AST snapshots are compared.

The [toolchain coverage record](toolchain-coverage.json) records per-directory
counts, the 372 newly verified skip removals, and source/frontend/oracle/runner
fingerprints. The default enabled directory set equals this verified scope.
The [earlier standard-library record](stdlib-coverage.json) remains a historical
428-directory/4,636-file snapshot. It also records the focused file-acceptance
samples. Both reports describe acceptance checks, not complete Go grammar or
language-server parity.

The remaining **298 exact skips all concern testdata** and require per-file
positive/negative classification. These are unclassified inventory, not verified
unsupported syntax. Separate `stdlib` and `toolchain` modes bypass the inventory,
so a future accidental skip cannot hide a failure in those scopes.


[Numeric validation follow-up](numeric-validation.json) records 118 expression
boundaries/underscore mutations (52 accepted, 66 rejected) and 354 complete files
(156 accepted, 198 rejected), with matching native acceptance. File contexts
cover package initializers, array lengths and nested function bodies. All 52
accepted expression token streams match go/scanner kinds, spans and semicolons.
The same 800-directory/6,821-file toolchain gate passes after numeric validation;
its source hash is unchanged. The record pins the new frontend and runner hashes.
Invalid numbers yield `Illegal`, so exact malformed token recovery and diagnostic
message parity with Go are outside this check. Numeric legality and typing remain
semantic work; this evidence does not classify the remaining testdata inventory.


[Unicode validation follow-up](unicode-validation.json) pins Go 1.27.3 / Unicode
17.0.0 and checks both identifier positions through the public lexer for all
**1,112,064 Unicode scalar values**. The oracle supplies Go's original strided
Letter/Decimal Digit category ranges; production uses compact contiguous ranges.
Twenty-two boundary files agree with go/parser (10 accepted, 12 rejected), and
accepted token streams agree with go/scanner. Regeneration is checked by the
maintainers' local tooling; normal Rust builds use the checked-in tables. The
final 800-directory/6,821-file toolchain gate still passes.

The [Unicode LSP comparison](../../benchmarks/unicode-identifiers.json)
matches 7/7 complete definition/reference responses with gopls, including non-BMP
letters, decimal-digit continuations, UTF-16 ranges and edit/close invalidation.
Existing completion regressions also pass after sharing identifier predicates
with completion prefix, qualifier and replacement-boundary scanning. This does
not establish every Unicode-sensitive semantic operation or malformed recovery.


[Import-string validation follow-up](import-validation.json) records 27 complete
stdio definition/reference queries across 16 import forms, all matching gopls.
Coverage includes octal/hex/Unicode escapes, escaped separators, raw-string CR
removal versus literal backslashes, inferred/explicit aliases, dot/blank imports,
invalid escapes/UTF-8, unrelated packages and unsaved edit/close invalidation.
Only decoded UTF-8 paths enter stub identity metadata. Ordinary literals stay
undecoded, and invalid UTF-8 import values are skipped without changing the
full-file grammar acceptance path. No parsed AST output is compared.

The full 800-directory/6,821-file toolchain gate still passes after sharing escape
validation with the lexer. Full Go semantic import-path validity checks and
default-name resolution for unindexed dependencies remain unfinished. The
record pins the frontend, oracle, runner and new stdio fixture fingerprints.


[Declared package-name validation](package-name-validation.json) records **20/20
complete feature responses matching gopls**, including definition/reference
locations, two negotiated semantic-token streams and prepare/outgoing call
hierarchy. Implicit names follow package clauses rather than path suffixes;
explicit aliases stay stable across unsaved clause changes. Ordinary production
names ending `_test` remain importable; external `_test.go` packages do not choose
the default name. Close restores disk metadata. Existing stdio goldens still pass.

The workspace stores package-clause name counts per canonical import path and
updates only changed file membership/names. Stubs retain explicit aliases or None,
without duplicate inferred-name strings. No sources, body arenas or query caches
are added to persistent metadata. The full 800-directory/6,821-file toolchain gate
still passes after the typed import metadata change. Unindexed dependencies retain
a lexical suffix fallback; dependency discovery, full import legality, systematic
testdata classification and broader semantic correctness remain unfinished.

## Full positive/negative classification

The 2026-11-08 all-scope run covers 1,098 directories and 8,081 files from the pinned source: 8,036 accepted and 45 rejected by go/parser. Both categories must match validate_file acceptance; accepted files retain all token and component checks. All 298 prior package skips were removed. The per-file evidence is [parser-all-files.json](../../benchmarks/parser-all-files.json). Use GOQUERY_GO_SCOPE=all and GOQUERY_GO_REPORT=<path> with the ignored go_source_package_corpus test to reproduce. No AST snapshots are compared. Deep nesting limits and exhaustive malformed-input fuzzing remain explicit gaps.
