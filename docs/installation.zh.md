# 安装

[English](installation.md) | 简体中文

使用较新的 Rust 工具链（edition 2024，Rust 1.85+）从源码构建：

```sh
cargo build --release
```

二进制位于 `target/release/goquery`（Windows 为 `goquery.exe`）。

Go 工具链是可选的：`go env` 提供默认的 `GOROOT`/`GOMODCACHE` 路径，`gofmt` 负责格式化。
没有 `go` 时，goquery 会回退到 `GOROOT`、`GOMODCACHE`/`GOPATH` 以及常见的 Go 安装布局，
因此无需在编辑器里配置 Go 路径。服务器不会下载依赖；如果模块缓存尚未准备好，请先运行
`go mod download`。

## Zed

Zed 没有直接替换 Go 语言服务器的设置：Go 绑定到名为 `gopls` 的内置 adapter。通过
`lsp.gopls.binary.path`（绝对路径）覆盖该 adapter 启动的二进制。把下面的配置加到 Windows 的
`%APPDATA%\Zed\settings.json`，或加到项目的 `.zed/settings.json` 以只对单个工作区生效：

```jsonc
{
  "lsp": {
    "gopls": {
      "binary": {
        // 绝对路径，例如 Windows 上的 C:\\path\\to\\goquery.exe。
        "path": "/path/to/goquery"
      }
    }
  }
}
```

- `binary.path` 必须是绝对路径。设置后 Zed 会跳过 `which("gopls")`、`go install ...gopls`
  回退以及它自己的下载路径。
- 不需要 `initialization_options`：goquery 启动时执行一次 `go env` 来发现 `GOROOT` 与
  `GOMODCACHE`。只有需要固定非默认 SDK 时才设置 `"goroot"`；在 hermetic 工作区中传
  `"goroot": ""` 可禁用 SDK 发现。
- 该模式下 Zed 不传参数；goquery 本来也忽略参数，所以 `["-mode=stdio"]` 同样可用。
- Zed 的状态栏与日志仍显示 `gopls`；实际运行的进程是 `goquery`。
- Zed 支持动态注册 `workspace/didChangeWatchedFiles`，goquery 借此发现外部
  （agent/shell）改动，并刷新受影响打开文档的诊断。即使编辑器不发送文件事件，
  服务器文件监控也能发现直接改动，没有轮询回退。
  `initializationOptions.fileWatch: false` 可关闭监控；使用 `fileWatchMax*`
  调整预算，使用 `fileWatchExcludeDirectories` 配置正则排除目录。
  干净的打开缓冲区会从磁盘重新加载；
  含未保存 `didChange` 修改的缓冲区仍以编辑器内容为准。
- Zed 会把 gopls 默认值（`usePlaceholders`、`hints`、`codelenses`、`semanticTokens`）合并进
  `initialization_options`；goquery 忽略不认识的键。
- Zed 中 Go 的 `format_on_save` 默认是 `on`，保存时会调用 goquery 的 `gofmt` 集成。可在
  `"languages": { "Go": ... }` 下设置 `"format_on_save": "off"` 关闭。
- 可选的、能启用 goquery 功能的设置：`"semantic_tokens": "combined"`、
  `"document_symbols": "on"`、`"document_folding_ranges": "on"`，以及全局
  `"code_lens": "on"`。

初始化选项见[配置](configuration.zh.md)。
