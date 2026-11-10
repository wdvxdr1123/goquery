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
| `GOQUERY_DISABLE_DIAG` | Disable pushed diagnostics. |
