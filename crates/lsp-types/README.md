# lsp-types patch

This directory is a copy of [`lsp-types` 0.94.1](https://crates.io/crates/lsp-types/0.94.1)
(MIT) plus **one field change**, used through `[patch.crates-io]` in the workspace
root. Do not edit other files: keep them byte-identical to the upstream release so
future updates remain a one-file merge.

## Why

`tower-lsp 0.20` fixes the `LanguageServer::document_link` return type to
`Option<Vec<lsp_types::DocumentLink>>`, so the server cannot substitute its own
`DocumentLink` type at that boundary. Upstream declares:

```rust
pub target: Option<url::Url>,
```

gopls emits document-link targets that are not always valid absolute URLs and
keeps their exact spelling. Across the recorded five-project differential corpus
(247 unique targets), 2 targets are rejected by `url::Url` (`https://::1`,
`https://2001:db8::1`) and 17 more are normalized (trailing slash, lowercased
host, IDN punycode, default-port spelling). Round-tripping through `Url` would
therefore change or drop links.

The patch replaces only that field:

```rust
pub target: Option<String>,
```

Cargo patches operate at package granularity, so the complete crate is
necessarily present here; only `src/document_link.rs` differs from upstream.
The raw string is the exact LSP wire value, which the document-link differential
compares byte-for-byte.
