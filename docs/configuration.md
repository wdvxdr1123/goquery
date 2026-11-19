# Configuration

[English](configuration.md) | [简体中文](configuration.zh.md)

All `initializationOptions` are optional. Unknown keys are ignored, so gopls
option dictionaries are accepted as-is; a known key with the wrong type fails
initialization with an error.

In Zed, place them under `lsp.gopls.initialization_options`. goquery reads the
LSP `initializationOptions`, not the dynamic `workspace/configuration` channel:

```jsonc
{
  "lsp": {
    "gopls": {
      "binary": {
        "path": "/path/to/goquery"
      },
      "initialization_options": {
        "goroot": "C:\\Program Files\\Go"
      }
    }
  }
}
```

## Options

The stdio runtime can read cancellation notifications while a single background
thread processes workspace changes and queries in order. Read-ahead pauses at
eight admitted messages or 1 MiB of admitted LSP body bytes. A frame can cross
the byte threshold and the next decoded frame can wait for admission.
Document highlighting checks for cancellation between regions and candidate
matches. Other active queries finish their current computation before the worker
discards a cancelled result. These limits are fixed.

| Key | Type | Description |
| --- | --- | --- |
| `goroot` | string | Optional SDK override. Absolute Go SDK root containing `src/builtin/builtin.go`. `""` disables SDK discovery. Default: `go env GOROOT`, then `GOROOT`, then the installation layout of `go` on `PATH`. |
| `goos` | string | Target OS for build-constraint selection. Default: `GOOS` or host. |
| `goarch` | string | Target architecture. Default: `GOARCH` or host. |
| `goexperiment` | string | Compiler experiment context. `""` selects the pinned baseline; experiment names are accepted as-is (unknown names become `goexperiment.<name>` tags). Default: `GOEXPERIMENT`. |
| `buildTags` | string[] | Extra build tags enabled for constraint evaluation. |
| `usePlaceholders` | bool | Insert snippet placeholders in function/method completions when the client supports snippets. Default `false`. |
| `completeFunctionCalls` | bool | Insert call syntax on function/method completion. Default `true`. |
| `completionDocumentation` | bool | Attach documentation to completion items. Default `true`. |
| `semanticTokens` | bool | Enable full and range semantic tokens. Default `true`. With `false`, the server omits `semanticTokensProvider` at initialization and returns `null` for token requests. Restart the server to apply changes. |
| `documentHighlight` | bool | Enable symbol occurrence highlights in the current document. Default `true`. With `false`, the server omits `documentHighlightProvider` at initialization and returns `null` for highlight requests. Restart the server to apply changes. |
| `diagnostics.onSave` | bool | Compute and push diagnostics on `textDocument/didSave`. Default `true`. Set to `false` for live diagnostics on open, edit, and file events. Restart the server to apply changes. |
| `fileWatch` | bool | Enable the server file watcher (Windows directory notifications, Linux inotify, macOS FSEvents). Default `true`; `false` disables it without enabling polling. |
| `fileWatchMaxRoots` | integer | File watch root budget after merging overlapping roots. Default `128`; `0` removes this application limit. |
| `fileWatchMaxDirectories` | integer | Directory watch budget on Linux and other nonrecursive backends. Default `4096`; `0` removes this application limit. Unused on Windows/macOS. |
| `fileWatchMaxEntries` | integer | Entry budget for file-watcher directory discovery on Linux and other nonrecursive backends. Default `20000`; `0` removes this application limit. Unused on Windows/macOS. |
| `fileWatchExcludeDirectories` | string[] | Regexes matching relative directory ancestor prefixes with `/` separators. Matching a directory excludes its subtree from file watching and event-triggered disk checks. Default `[]`. |

Save-only diagnostics use the nested option `"diagnostics": {"onSave": true}`.
Automatic saves also send `didSave`. In Zed, set the top-level editor option
`"autosave": "off"` to disable periodic or focus-triggered autosaves.
`GOQUERY_DISABLE_DIAG` takes precedence over the trigger setting.

File events wake a task with one coalesced pending flag and a 100 ms burst debounce.
There is no periodic scan, request-triggered poll or polling fallback. Errors are
logged; correct the config/system limit and restart to retry registration. Some
network/virtual filesystems silently omit events; these require editor file events
or editor open/change/save notifications to refresh.
Windows and macOS (FSEvents) monitor roots recursively, merging overlapping roots.
Linux and other nonrecursive backends enumerate directories, excluding ignored
directories, and register new directories after reconciliation. The three file
watch budgets above are configurable; exceeding an enabled budget logs the option
to raise and disables server watching. For a large workspace, increase the budgets
or use the following initialization options to remove the application limits:

```json
{
  "fileWatch": true,
  "fileWatchMaxRoots": 0,
  "fileWatchMaxDirectories": 0,
  "fileWatchMaxEntries": 0,
  "fileWatchExcludeDirectories": ["(^|/)(generated|cache)$", "^build-output$"]
}
```

These options only control file-watcher registration, not indexing or disk-scan
budgets. Removing them allows watch storage to grow with the workspace; operating
system limits still apply. Linux limits are visible with
`sysctl fs.inotify.max_user_watches fs.inotify.max_user_instances fs.inotify.max_queued_events`.
They are system settings and goquery does not change them. No event-path queue or
source cache is retained.
Exclusion regexes match each ancestor directory prefix relative to the first
covering workspace root; filenames are not matched. Matching is case-sensitive
(`(?i)` overrides). Initial indexing and editor events are unchanged. Built-in
ignored directories remain ignored. On Windows/macOS recursive OS subscriptions
still cover excluded paths, filtering after delivery. At most 128 regexes and
65,536 UTF-8 pattern bytes are accepted, with 1 MiB compilation and lazy DFA
budgets each. Invalid/oversized regexes fail initialization.
Each disk scan visits at most 20,000 entries and reports at most 4,096 changed
paths. It reuses directory-entry metadata for indexed files; indexed paths absent
from the walk are checked separately, including deletions. Clean open buffers are
reloaded from disk, while dirty editor buffers stay authoritative.
Explicit rescan events (including queue overflow) trigger one bounded scan.
New files beyond the traversal budget may remain undiscovered.
Retired `pollIntervalMs`, `GOQUERY_POLL_MS` and `GOQUERY_ONDEMAND_MAX_FILES` settings
are ignored.

The module cache has no initialization option. It is discovered from
`go env GOMODCACHE`, then `GOMODCACHE`, then the first `GOPATH` entry
(`<GOPATH>/pkg/mod`), then `~/go/pkg/mod`. At most one `go env` probe runs at
initialization with `GOTOOLCHAIN=local` and `GOWORK=off`; it is time-bounded and
never fails initialization: a missing or failing `go` binary simply leaves the
environment fallbacks in place.

## Source and orphan-view limits

Opening a build-excluded Go file can admit up to 64 compatible production
siblings, with 8 MiB of source reads. Its offline dependency view shares a limit
of 16 additional files, 4 MiB of reads, 128 import edges and 16 KiB of import
paths. Each admission directory is limited to 2,048 entries. These are fixed
limits; they do not launch Go or download dependencies. Queries use admitted
metadata rather than discovering packages on disk. Missing or exhausted facts
remain unknown, and incomplete dependency admission is logged to stderr.

Hover constant-text expansion is limited to 4,096 steps, 64 KiB of declaration
reads, depth 16 and 16 KiB of expanded string bytes. Package documentation reads
only a 64 KiB header. These limits can omit presentation details without
inventing a type or a declaration target.

Completion ranks at most 200 candidates before formatting selected member types;
their native type strings together are limited to 16 KiB. Type declaration
presentation uses the shared arena with at most 256 temporary nodes; each type
view and each source header for parameter names is limited to 16 KiB. Large
struct/interface declarations can display their known outer constructor without
expanding all members. Exhaustion omits type details rather than guessing them.
Native member enumeration also limits copied candidate names to 16 KiB and
keeps private names distinct by declaring package; an exhausted method set stays
unknown.
Receiver metadata rejects unrelated method candidates before native signature
checking. Alias and dot-import receivers still require exact native identity.
The temporary alias inventory and surviving method candidates each have a
4,096-entry ceiling; neither is retained beyond the query.
Implementation candidates with public requirements resolve only the required
native methods; each projection retains the existing 256-node and 16 KiB
budgets. Private requirements retain the full package-qualified method inventory.
Final implementation proofs still check every interface requirement.
Each checked region bounds expected-expression and literal-construction context
tables to 4,096 records apiece; overflow discards the entire affected table.
Completion applies native assignment compatibility within its 200-item fuzzy
preselection. Unknown compatibility keeps the existing score. Field-key snippets
use checked literal fields and omit keys already present in the literal.
Module language versions are stored with the offline module plan and refresh on
manifest events. Leading build constraints select the per-file language using
Go's 1.21 floor. Go 1.27 literal keys include unambiguous promoted fields without
pointer indirection; already used ancestor or descendant paths exclude a key.
This adds one compact release value per indexed file and per admitted module;
queries do not reopen manifests.
Native signatures retain parameter/result names as presentation metadata, with
at most one record per arena node and a shared 16 KiB name pool. Generic
substitution and method expressions preserve these names; arena rewinds discard
them. Exhausting this pool omits names without changing semantic type facts.
Type metadata records exact parameter-list and RHS offsets. Oversized trivia
between these parts does not force a whole-file read: parameters and the actual
RHS are read separately, each within the existing 16 KiB declaration limit.
Substantive type syntax beyond that limit remains unknown.
Function bodies are admitted within 128 KiB, with 16 KiB signature headers.
The request facade shares its current parsed body with cursor candidates and
releases it between streaming regions. Body-budget exhaustion yields no local
semantic result or completion candidates, since lexical shadowing is unknown.
Every region transition rewinds local formals/types. Up to 64 newly used global
declarations are recreated before the next body and retained below its permanent
watermark; declaration-cache and arena ceilings still apply. Cache pressure
evicts this prefix rather than consuming the next function's local budgets.

## Environment switches

Read once at startup:

| Variable | Effect |
| --- | --- |
| `GOQUERY_TRACE_QUERIES` | Log reference/call candidate and file counts to stderr. |
| `GOQUERY_TRACE_MODULES` | Log module-graph planning. |
| `GOQUERY_TRACE_EDITS` | Log each workspace mutation, its affected-document count and elapsed milliseconds to stderr (useful for diagnosing stale feedback after agent/editor changes). |
| `GOQUERY_DISABLE_DIAG` | Disable pushed diagnostics. |
