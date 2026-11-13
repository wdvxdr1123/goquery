# Installation

[English](installation.md) | [简体中文](installation.zh.md)

Build from source with a recent Rust toolchain (edition 2024, Rust 1.85+):

```sh
cargo build --release
```

The binary is `target/release/goquery` (`goquery.exe` on Windows).

A Go toolchain on `PATH` is optional: `go env` supplies the default
`GOROOT`/`GOMODCACHE` paths, and `gofmt` powers formatting. Without `go`, goquery
falls back to `GOROOT`, `GOMODCACHE`/`GOPATH` and the usual Go installation layout,
so no Go paths need to be configured in the editor. The server never downloads
dependencies; run `go mod download` ahead of time if the cache is not ready.

## Zed

Zed has no setting that swaps Go's language server: Go is bound to a built-in
adapter named `gopls`. Override the binary that adapter launches through
`lsp.gopls.binary.path` (absolute path). Add this to `%APPDATA%\Zed\settings.json`
on Windows, or to a project's `.zed/settings.json` to scope it to one workspace:

```jsonc
{
  "lsp": {
    "gopls": {
      "binary": {
        // Absolute path, for example C:\\path\\to\\goquery.exe on Windows.
        "path": "/path/to/goquery"
      }
    }
  }
}
```

- `binary.path` must be absolute. Setting it makes Zed skip `which("gopls")`, the
  `go install ...gopls` fallback and its own download path.
- No `initialization_options` are required: goquery runs `go env` once at startup
  to discover `GOROOT` and `GOMODCACHE`. Set `"goroot"` only to pin a non-default
  SDK, and pass `"goroot": ""` to disable SDK discovery in a hermetic workspace.
- Zed passes no arguments in this mode; goquery ignores arguments either way, so
  `["-mode=stdio"]` is also accepted.
- Zed still labels the server `gopls` in the status bar and logs; the running
  process is `goquery`.
- Zed supports dynamic `workspace/didChangeWatchedFiles` registration, which
  goquery uses to notice external (agent/shell) edits and refresh diagnostics
  for affected open documents. The server file watcher also notices direct edits
  without editor events; there is no polling fallback. `initializationOptions.fileWatch: false`
  disables it. Configure budgets and regex directory exclusions with `fileWatchMax*`
  and `fileWatchExcludeDirectories`. Clean buffers reload from disk; buffers
  with unsaved `didChange` edits stay authoritative.
- Zed merges its gopls defaults (`usePlaceholders`, `hints`, `codelenses`,
  `semanticTokens`) into `initialization_options`. goquery ignores the keys it
  does not know.
- Go's per-language `format_on_save` default is `on` in Zed, so saving invokes
  goquery's `gofmt` integration. Set `"format_on_save": "off"` under
  `"languages": { "Go": ... }` to disable it.
- Optional settings that expose goquery features: `"semantic_tokens": "combined"`,
  `"document_symbols": "on"`, `"document_folding_ranges": "on"`, and the global
  `"code_lens": "on"`.

Continue with [configuration](configuration.md) for initialization options.
