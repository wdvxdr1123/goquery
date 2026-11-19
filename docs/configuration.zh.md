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

工作区更新和查询由一个后台线程按顺序处理，stdio 运行时可以同时接收取消通知。
已接收消息达到 8 条或消息体累计达到 1 MiB 时，暂停预读。触发字节上限的单条消息可以跨过阈值，
下一条已解码消息可以等待入队。文档高亮在扫描区域和匹配候选之间检查取消状态。
其他执行中的查询会完成当前计算，再丢弃已取消的结果。这些上限固定。

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
| `semanticTokens` | bool | 启用整文件和范围语义 token，默认 `true`。设为 `false` 时，初始化响应省略 `semanticTokensProvider`，语义 token 请求返回 `null`。更改后重启服务器。 |
| `documentHighlight` | bool | 启用当前文档中同一符号的出现位置高亮，默认 `true`。设为 `false` 时，初始化响应省略 `documentHighlightProvider`，文档高亮请求返回 `null`。更改后重启服务器。 |
| `diagnostics.onSave` | bool | 收到 `textDocument/didSave` 时计算并推送诊断，默认 `true`。设为 `false` 时恢复打开、编辑及文件事件触发的实时诊断。更改后重启服务器。 |
| `fileWatch` | bool | 启用服务器文件监控（Windows 目录通知、Linux inotify、macOS FSEvents），默认 `true`。设为 `false` 关闭监控，不启用轮询。 |
| `fileWatchMaxRoots` | integer | 合并重叠根目录后的文件监控根目录预算，默认 `128`；`0` 取消这项应用层限制。 |
| `fileWatchMaxDirectories` | integer | Linux 等非递归后端的目录监控预算，默认 `4096`；`0` 取消这项应用层限制。Windows/macOS 不使用。 |
| `fileWatchMaxEntries` | integer | Linux 等非递归后端发现监控目录时的遍历条目预算，默认 `20000`；`0` 取消这项应用层限制。Windows/macOS 不使用。 |
| `fileWatchExcludeDirectories` | string[] | 排除目录的正则数组，匹配使用 `/` 分隔的相对目录祖先路径。匹配目录后排除整棵子树的文件事件和事件触发的磁盘核对，默认 `[]`。 |

保存时诊断使用嵌套选项 `"diagnostics": {"onSave": true}`。自动保存也会发送
`didSave`。在 Zed 的顶层编辑器设置中设为 `"autosave": "off"`，即可关闭定时或焦点变化触发的自动保存。
`GOQUERY_DISABLE_DIAG` 的优先级高于诊断触发设置。

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

## 源码与孤立文件视图预算

打开被构建条件排除的 Go 文件时，最多准入 64 个兼容的生产同包文件，源码读取累计不超过 8 MiB。
其离线依赖视图共享 16 个新增文件、4 MiB 读取、128 条导入边和 16 KiB 导入路径预算，
每个准入目录最多检查 2,048 个条目。这些限制不可配置；整个过程不调用 Go 或下载依赖。
语义查询使用已准入元数据，不在磁盘上发现包；缺失或超预算的事实保持未知，依赖准入不完整时记录 stderr 日志。

Hover 常量文本展开限制为 4,096 步、64 KiB 声明读取、深度 16 和 16 KiB 展开字符串字节。
包文档只读取 64 KiB 文件头；超限可省略展示信息，不会据此编造类型或声明目标。

补全先将候选排名截断到 200 项，再渲染选中的成员类型；这些原生类型字符串合计限制为 16 KiB。
类型声明展示复用共享 Arena，最多新增 256 个临时节点；单个类型视图和用于附着参数名的源码头各限 16 KiB。
大型 struct/interface 声明可直接展示已知外层构造，无需展开全部成员；预算耗尽时省略类型详情。
原生成员枚举复制的候选名称也合计限制为 16 KiB，私有名称按声明包区分；方法集预算耗尽时保持未知。
接收者元数据先排除无关方法候选，再检查原生签名；别名和 dot import 接收者仍须核验精确身份。
临时别名清单和保留的方法候选各限 4,096 项，不跨请求保留。
实现候选只按所需公有方法做原生选择，投影仍限 256 节点和 16 KiB；
私有要求仍保留完整的包身份方法清单，最终证明始终核验接口的全部要求。
每个检查区域的表达式期望和字面量构造上下文各限 4,096 条，溢出时丢弃整张对应表。
补全在已有 200 项模糊匹配预选内按原生赋值兼容性调整权重，未知兼容性保留原分数。
字段键 snippet 使用已检查的字面量字段，并排除字面量中已有的键。

模块语言版本随离线模块计划保存，由 manifest 事件刷新；文件构建约束按 Go 的
1.21 下限选择文件语言。Go 1.27 字面量键支持无歧义、无指针间接路径的提升字段，
已有父/子字段路径会排除冲突候选。每个索引文件及已准入模块仅增加一个紧凑版本值，
查询不重新打开 manifest。
原生签名的参数/结果名仅作为展示元数据保存，每个 Arena 节点最多一条记录，
共用 16 KiB 名称池。泛型替换和方法表达式保留名称，Arena 回滚同步释放；
名称池耗尽仅省略参数名，不改变语义类型事实。
类型元数据记录精确的参数列表末尾和 RHS 起点。两者之间的超长注释不触发
整文件读取：参数头和实际 RHS 分别遵守既有 16 KiB 声明限制，超限的实际
类型语法仍保持未知。
函数体统一限 128 KiB，签名头仍限 16 KiB。请求门面与光标候选共享当前函数体
解析结果，流式区域之间释放。体预算耗尽时，局部语义及补全候选保持空/未知，
避免在无法确定词法遮蔽时误报全局对象。
每次区域切换都回滚局部 formal 和局部类型。新使用的全局声明最多 64 个在下个
函数体之前重新实体化，保存在永久水位下；仍受声明缓存及 Arena 总预算限制。
缓存压力过高时淘汰该前缀，避免挤占下一个函数的局部预算。

## 环境变量开关

启动时读取一次：

| 变量 | 作用 |
| --- | --- |
| `GOQUERY_TRACE_QUERIES` | 向 stderr 输出引用/调用候选与文件计数。 |
| `GOQUERY_TRACE_MODULES` | 输出模块图规划日志。 |
| `GOQUERY_TRACE_EDITS` | 向 stderr 输出每次工作区变更、受影响文档数与耗时（毫秒），用于诊断 agent/编辑器改动后的反馈延迟。 |
| `GOQUERY_TRACE_LSP` | 向 stderr 输出方法、请求或取消 ID、入队后等待时间、处理耗时和响应错误码。处理耗时截至结果交给 stdout 之前。 |
| `GOQUERY_DISABLE_DIAG` | 禁用推送诊断。 |
