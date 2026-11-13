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
| `fileWatch` | bool | 启用服务器文件监控（Windows 目录通知、Linux inotify、macOS FSEvents），默认 `true`。设为 `false` 关闭监控，不启用轮询。 |
| `fileWatchMaxRoots` | integer | 合并重叠根目录后的文件监控根目录预算，默认 `128`；`0` 取消这项应用层限制。 |
| `fileWatchMaxDirectories` | integer | Linux 等非递归后端的目录监控预算，默认 `4096`；`0` 取消这项应用层限制。Windows/macOS 不使用。 |
| `fileWatchMaxEntries` | integer | Linux 等非递归后端发现监控目录时的遍历条目预算，默认 `20000`；`0` 取消这项应用层限制。Windows/macOS 不使用。 |
| `fileWatchExcludeDirectories` | string[] | 排除目录的正则数组，匹配使用 `/` 分隔的相对目录祖先路径。匹配目录后排除整棵子树的文件事件和事件触发的磁盘核对，默认 `[]`。 |

文件事件唤醒后台任务，并合并为一个待检查标志，收到事件后延迟 100 ms 合并连续写入。
没有定时扫描、请求触发的按需轮询或轮询回退。错误只记录日志，调整配置或系统限制后重启服务器重新注册。
部分网络盘、虚拟文件系统静默不发送事件，此时需要编辑器文件事件，或编辑器打开、修改、保存来刷新。
Windows 和 macOS（FSEvents）按根目录递归监控，重叠根目录会合并。
Linux 等非递归后端会枚举目录，跳过忽略目录，并在核对后注册新目录。上面三项文件监控预算均可配置；
超过启用的预算时停止服务器监控，并在日志中给出应调整的选项。大工作区可以提高预算，或使用以下初始化选项
取消应用层数量限制：

```json
{
  "fileWatch": true,
  "fileWatchMaxRoots": 0,
  "fileWatchMaxDirectories": 0,
  "fileWatchMaxEntries": 0,
  "fileWatchExcludeDirectories": ["(^|/)(generated|cache)$", "^build-output$"]
}
```

这些选项只控制文件监控注册，不改变索引或磁盘扫描的独立预算。取消限制后，监控存储会随工作区规模增长；
操作系统限制仍有效。Linux 可通过
`sysctl fs.inotify.max_user_watches fs.inotify.max_user_instances fs.inotify.max_queued_events`
查看系统限制；goquery 不修改这些系统设置。不保留事件路径队列或源码缓存。
排除正则按第一个覆盖路径的工作区根目录计算相对目录路径，统一使用 `/`，逐级匹配祖先目录。
匹配目录后排除子树；文件名不参与匹配。默认区分大小写，可用 `(?i)` 覆盖。
排除项不改变初始化索引或编辑器事件处理。内置忽略目录仍有效。
Windows/macOS 系统递归订阅仍覆盖排除路径，在事件送达后过滤；Linux 在注册时跳过。
最多接受 128 个正则、共 65,536 字节 UTF-8 模式；编译与惰性 DFA 缓存预算各 1 MiB。
无效或超预算的正则会使初始化失败。
每次磁盘扫描最多遍历 20,000 个条目、报告 4,096 个变更路径，
复用目录条目的元数据检查已索引文件；未出现在遍历中的已索引路径仍单独检查，包括删除。
干净的打开缓冲会从磁盘重载，有未保存修改的编辑器缓冲仍以编辑器为准。
明确要求重新扫描的事件（包括队列溢出）会触发一次有界核对，超出遍历预算的新文件可能尚未被发现。
已移除的 `pollIntervalMs`、`GOQUERY_POLL_MS` 和 `GOQUERY_ONDEMAND_MAX_FILES` 设置不再生效。

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
| `GOQUERY_DISABLE_DIAG` | 禁用推送诊断。 |
