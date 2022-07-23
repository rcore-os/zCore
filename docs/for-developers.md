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

仓库更新工具链的周期约为 9 周，但可能随着某些重要的不稳定特性变化而调整，或随着重要的其他 pr 一起合并。下一次预定的更新时间为 **2022 年 9 月 21 日**。在此之前，也欢迎到[讨论区](https://github.com/rcore-os/zCore/discussions/356)更新已知的兼容性问题。

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

todo

## 依赖管理策略

todo

## features and cfg

todo
