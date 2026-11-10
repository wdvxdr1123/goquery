# goquery

[English](README.md) | [简体中文](README.zh.md)

A lightweight Go language server written in Rust. It aims to be a compact,
low-memory alternative to `gopls` for day-to-day editing and AI-assisted
development; the feature scope follows ccls and is adapted to Go semantics, with
gopls as the semantic reference.

Design goals:

- Reliable cross references, hierarchies, semantic navigation and semantic
  highlighting first; completion is maintained but lower priority than semantic
  correctness.
- Substantially consistent with gopls on the Go standard library and some large
  Go projects; differences are recorded rather than hidden.
- Large workspaces stay usable: full-project sessions on large Go projects must
  keep the server root process below 100 MB in both OS peak working set and
  sampled private bytes, counted separately from any Go child process.
- No Go or gopls invocation in the semantic path.

The server is currently experimental. Most of the feature set is implemented, but
full type/expression resolution, complete build selection and complete
interface/type-set semantics remain unfinished.

## Documentation

- [Installation](docs/installation.md) ([中文](docs/installation.zh.md)): build
  goquery and Zed configuration.
- [Configuration](docs/configuration.md) ([中文](docs/configuration.zh.md)):
  initialization options and environment switches.

## AI use disclaimer

This project makes heavy use of LLM-assisted development.

## License

goquery itself is licensed under the BSD 3-Clause License in [LICENSE](LICENSE).
Portions adapted from Go and gopls (notably the fuzzy symbol/completion matchers)
remain under the Go Authors' BSD-3-Clause license in [LICENSE-GO](LICENSE-GO).

## Acknowledgements

- [gopls](https://github.com/golang/tools) is the behavioral and semantic
  reference; completion scoring, fuzzy symbol matching, hover/documentation
  presentation, default code lenses and similar algorithms are adapted from it.
- Go standard library and toolchain sources are the primary corpus and
  correctness baseline.
- [tower-lsp](https://github.com/ebkalderon/tower-lsp),
  [tokio](https://github.com/tokio-rs/tokio),
  [lsp-types](https://github.com/gluon-lang/lsp-types),
  [serde](https://github.com/serde-rs/serde) and
  [walkdir](https://github.com/BurntSushi/walkdir) provide the protocol, runtime,
  serialization and directory-traversal foundations.
