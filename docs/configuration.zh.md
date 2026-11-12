# 配置

[English](configuration.md) | 简体中文

所有 `initializationOptions` 都是可选的。未知键会被忽略，因此可以直接传入 gopls 的选项字典；
已知键类型错误会导致初始化失败。

在 Zed 中，把它们放在 `lsp.gopls.initialization_options` 下。goquery 读取 LSP 的
`initializationOptions`，不读取动态的 `workspace/configuration` 通道：

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

## 选项

| 键 | 类型 | 说明 |
| --- | --- | --- |
| `goroot` | string | 可选的 SDK 覆盖。必须是包含 `src/builtin/builtin.go` 的 Go SDK 根目录。`""` 表示禁用 SDK 发现。默认依次取 `go env GOROOT`、`GOROOT`，再按 `PATH` 上 `go` 的安装布局推断。 |
| `goos` | string | 用于 build 约束选择的目标 OS。默认 `GOOS` 或宿主系统。 |
| `goarch` | string | 目标架构。默认 `GOARCH` 或宿主架构。 |
| `goexperiment` | string | 编译器 experiment 上下文。`""` 选择固定的基线；experiment 名称原样接受（未知名称作为 `goexperiment.<name>` 标签）。默认 `GOEXPERIMENT`。 |
| `buildTags` | string[] | 参与约束求值的额外 build tags。 |
| `usePlaceholders` | bool | 客户端支持 snippet 时，在函数/方法补全中插入参数占位符。默认 `false`。 |
| `completeFunctionCalls` | bool | 函数/方法补全时插入调用语法。默认 `true`。 |
| `completionDocumentation` | bool | 给补全项附加文档。默认 `true`。 |
| `pollIntervalMs` | integer | 直接磁盘轮询间隔（毫秒，`0` 关闭）。默认取 `GOQUERY_POLL_MS`，未设置时为 `1000`；限制在 100–60000 ms。检测到直接改动后会在 3 秒内临时改为 100 ms 轮询，以捕获 agent 连续写入的多个文件。无需编辑器事件即可发现改动，包括干净的打开缓冲、新文件与 manifest 变化；有未保存修改的编辑器缓冲仍以编辑器为准。 |

模块缓存没有初始化选项。它依次从 `go env GOMODCACHE`、`GOMODCACHE`、第一个 `GOPATH` 条目
（`<GOPATH>/pkg/mod`）、`~/go/pkg/mod` 发现。初始化时最多执行一次 `go env` 探测，带
`GOTOOLCHAIN=local` 与 `GOWORK=off`；该探测有时间上限，且绝不会导致初始化失败：缺少或执行失败的
`go` 二进制只会保留环境变量回退。

## 环境变量开关

启动时读取一次：

| 变量 | 作用 |
| --- | --- |
| `GOQUERY_TRACE_QUERIES` | 向 stderr 输出引用/调用候选与文件计数。 |
| `GOQUERY_TRACE_MODULES` | 输出模块图规划日志。 |
| `GOQUERY_TRACE_EDITS` | 向 stderr 输出每次工作区变更、受影响文档数与耗时（毫秒），用于诊断 agent/编辑器改动后的反馈延迟。 |
| `GOQUERY_POLL_MS` | 未设置 `pollIntervalMs` 时的默认直接磁盘轮询间隔（毫秒，默认 `1000`；`0` 关闭）。取值限制在 100–60000 ms。 |
| `GOQUERY_ONDEMAND_MAX_FILES` | 允许请求触发按需轮询的工作区文件数上限（默认 `512`；`0` 关闭）。仅当语义请求前已有 300 ms 空闲时触发一次。更大的树扫描代价过高，应改用更低的 `pollIntervalMs`/`GOQUERY_POLL_MS` 加快后台反馈。 |
| `GOQUERY_DISABLE_DIAG` | 禁用推送诊断。 |
