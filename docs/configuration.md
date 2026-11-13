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
| `fileWatch` | bool | Enable the server file watcher (Windows directory notifications, Linux inotify, macOS FSEvents). Default `true`; `false` disables it without enabling polling. |
| `fileWatchMaxRoots` | integer | File watch root budget after merging overlapping roots. Default `128`; `0` removes this application limit. |
| `fileWatchMaxDirectories` | integer | Directory watch budget on Linux and other nonrecursive backends. Default `4096`; `0` removes this application limit. Unused on Windows/macOS. |
| `fileWatchMaxEntries` | integer | Entry budget for file-watcher directory discovery on Linux and other nonrecursive backends. Default `20000`; `0` removes this application limit. Unused on Windows/macOS. |
| `fileWatchExcludeDirectories` | string[] | Regexes matching relative directory ancestor prefixes with `/` separators. Matching a directory excludes its subtree from file watching and event-triggered disk checks. Default `[]`. |

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

## Environment switches

Read once at startup:

| Variable | Effect |
| --- | --- |
| `GOQUERY_TRACE_QUERIES` | Log reference/call candidate and file counts to stderr. |
| `GOQUERY_TRACE_MODULES` | Log module-graph planning. |
| `GOQUERY_TRACE_EDITS` | Log each workspace mutation, its affected-document count and elapsed milliseconds to stderr (useful for diagnosing stale feedback after agent/editor changes). |
| `GOQUERY_DISABLE_DIAG` | Disable pushed diagnostics. |
