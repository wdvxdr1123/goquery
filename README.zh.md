# goquery

[English](README.md) | 简体中文

一个用 Rust 编写的轻量 Go 语言服务器。目标是成为 `gopls` 的紧凑、低内存替代品，服务于日常
编辑与 AI 辅助开发；功能范围参考 ccls 并按 Go 语义调整，语义行为以 gopls 为基准。

设计目标：

- 优先保证可靠的交叉引用、层级关系、语义导航与语义高亮；补全继续维护，但优先级低于语义正确性。
- 在 Go 标准库与一些大型 Go 项目上与 gopls 基本一致；差异会被记录，而不是隐藏。
- 大型工作区保持可用：大型 Go 项目的全项目会话必须让服务器根进程在 OS 峰值工作集与采样私有内存
  两项上都低于 100 MB，并与任何 Go 子进程分开统计。
- 语义路径中不调用 Go 或 gopls。

服务器目前是实验性的。功能集已实现大部分，但完整的类型/表达式解析、完整的 build 选择以及完整的
interface/type-set 语义仍未完成。

## 文档

- [安装](docs/installation.zh.md)（[English](docs/installation.md)）：构建 goquery 与 Zed 配置。
- [配置](docs/configuration.zh.md)（[English](docs/configuration.md)）：初始化选项与环境变量开关。

## AI 使用声明

本项目大量使用 LLM 辅助开发。

## 许可证

goquery 本体以 BSD 3-Clause 许可证发布，见 [LICENSE](LICENSE)。改编自 Go 与 gopls 的部分
（尤其是模糊符号/补全匹配器）沿用 Go Authors 的 BSD-3-Clause 许可证，见 [LICENSE-GO](LICENSE-GO)。

## 致谢

- [gopls](https://github.com/golang/tools) 是行为与语义基准；补全打分、模糊符号匹配、
  hover/文档展示、默认 code lens 等算法改编自它。
- Go 标准库与工具链源码是主要语料与正确性基准。
- [tower-lsp](https://github.com/ebkalderon/tower-lsp)、
  [tokio](https://github.com/tokio-rs/tokio)、
  [lsp-types](https://github.com/gluon-lang/lsp-types)、
  [serde](https://github.com/serde-rs/serde) 与
  [walkdir](https://github.com/BurntSushi/walkdir) 提供协议、运行时、序列化与目录遍历基础。
