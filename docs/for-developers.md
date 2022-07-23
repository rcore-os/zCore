# 开发者注意事项（草案）

本文用于同步开发 zCore 的一些策略选择并提醒开发者某些需要注意的信息。

> 本文暂时是一份草案，欢迎在相关微信群或 github [讨论区](https://github.com/rcore-os/zCore/discussions/356)发表对形式和内容的意见

## 目录

- [工具链支持策略](#工具链支持策略)
  > - 确定跟踪工具链的时间点
  > - 汇总使用到的不稳定特性，以及使用的原因
- [代码质量保障](#代码质量保障)
  > 介绍如何使用 `#[deny(...)]` 和 `#[allow(...)]`
- [依赖管理策略](#依赖管理策略)
  > 介绍何时应以何种方式引入依赖项
- [features and cfg](#features-and-cfg)
  > 介绍如何使用编译条件的写法

## 工具链支持策略

由于[下文](#各模块使用的不稳定特性)详述的具体原因，本项目依赖一些不稳定特性，只能使用 nightly 工具链编译。仓库将测试可用的工具链记录在 cargo 可识别的[工具链配置文件](../rust-toolchain.toml)，使用 cargo 构建项目时会自动安装可靠的工具链。

仓库使用的工具链虽然是 nightly 的，但将伴随 stable 工具链的版本更新而更新。每当 stable 工具链更新，就将默认工具链更新到最接近最新 stable 的版本，周期约为 6 周。参考[版本信息网站](https://forge.rust-lang.org/)记录的最新稳定版分叉发生时间，使用对应的 nightly 即可。

下一次更新的版本为 1.63（2022-08-05），进行更新的日期为 2022 年 8 月 11 日。

> 最新稳定版本 1.62（2022-06-24）。但由于需要同时编译的 rboot 项目已经更新到了 2022-07-20，所以目前就保持这个版本。
>
> 本文档记录的预期更新时间将随着每次工具链更新而更新。

### 使用的不稳定特性

#### [`doc_cfg`](https://doc.rust-lang.org/unstable-book/language-features/doc-cfg.html)

用到的模块：`zcore-drivers`、`kernel-hal`、`zcore-loader`

用于在文档上标记对象可使用的平台信息。

标注有助于提升文档质量，但不影响使用，必要时可去除。

#### [`naked_functions`](https://doc.rust-lang.org/unstable-book/language-features/naked-functions.html), [`asm_sym`](https://doc.rust-lang.org/unstable-book/language-features/asm-sym.html), [`asm_const`](https://doc.rust-lang.org/unstable-book/language-features/asm-const.html)

用到的模块：`zcore`

这三项用于支撑一个 rust 裸函数。裸函数不会自动插入栈操作，因此可用于设置栈之前的阶段。配合用于向内联汇编导入 rust 常量的 `asm_const` 和向内联汇编导入 rust 符号的 `asm_sym`，可以将整个启动阶段尽量置于 rust 的保护之下（避免硬编码常量或导出全局符号）。

可以移除，但不建议。

#### [`default_alloc_error_handler`](https://doc.rust-lang.org/unstable-book/language-features/default-alloc-error-handler.html)

用到的模块：`zcore`

要求 `alloc` 提供一个默认的分配失败回调。

只要同时使用 `no_std` 和 `alloc` 就必然需要分配失败回调，可选默认的或通过另一个不稳定特性 [`alloc-error-handler`](https://doc.rust-lang.org/unstable-book/language-features/alloc-error-handler.html) 提供一个自定义的。

## 代码质量保障

代码质量主要依赖 **clippy** 来保障。因此，需要创造利于 clippy 运行的环境，这包括三个部分：

1. rust-analyzer 绑定

如果使用 vs code 开发，rust-analyzer 支持 checkOnSave 功能，即保存时自动检查（得益于现代编译器的效率，这是可能的）。如果配合自动保存功能，可实现随时检查，只要将检查方式默认为 clippy，即可随时 clippy。

当然 clippy 运行比较慢，如果电脑性能不适应，只好关掉。

2. `#![deny(warnings)]`

这个选项将当前模块（当然也包括子模块）产生的警告视为错误，这会禁止产生警告的代码完成编译，有助于及早解决问题。clippy 也将产生警告，这些警告也被视作错误。

目前，所有模块都添加了这个选项（有些还禁止了别的东西，unsafe_code 及 missing_docs 等，这不在我们的讨论范围内），且处在可通过编译的状态。开发时为了方便当然可以注释掉它们，但如果发布 PR，务必恢复这些选项。

3. github actions

使用 clippy 检查代码已经写入 github 工作流，每次提交都会自动执行。这可以向其他开发者证明你的代码质量受到保障。可以在本地使用 `cargo check-style` 命令检查代码的合规性，其流程和使用 clippy 的方式与 github 工作流保持一致。

## 依赖管理策略

todo

## features and cfg

todo
