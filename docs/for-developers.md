# 开发者注意事项（草案）

本文用于同步开发 zCore 的一些策略选择并提醒开发者某些需要注意的信息。

> 本文暂时是一份草案，欢迎在相关微信群或 github [讨论区](https://github.com/rcore-os/zCore/discussions/356)发表对形式和内容的意见

## 目录

- [工具链支持策略](#工具链支持策略)
  >
  > - 确定跟踪工具链的时间点
  > - 汇总使用到的不稳定特性，以及使用的原因
  >
- [代码质量保障](#代码质量保障)
  > 介绍如何使用 `#[deny(...)]` 和 `#[allow(...)]`
- [依赖管理策略](#依赖管理策略)
  > 介绍何时应以何种方式引入依赖项
- [features and cfg](#features-and-cfg)
  > 介绍如何使用编译条件

## 工具链支持策略

由于[下文](#使用的不稳定特性)详述的具体原因，本项目依赖一些不稳定特性，只能使用 nightly 工具链编译。仓库将测试可用的工具链记录在 cargo 可识别的[工具链配置文件](../rust-toolchain.toml)，使用 cargo 构建项目时会自动安装可靠的工具链。

仓库使用的工具链虽然是 nightly 的，但将伴随 stable 工具链的版本更新而更新。每当 stable 工具链更新，就将默认工具链更新到最接近最新 stable 的版本，周期约为 6 周。参考[版本信息网站](https://forge.rust-lang.org/)记录的最新稳定版分叉发生时间，使用对应的 nightly 即可。

下一次更新的版本为 1.64（2022-08-05），进行更新的日期为 2022 年 9 月 22 日。

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

依赖从特殊到一般，分为以下 4 种形式：

1. [git 子模块](#git-子模块)
2. [个人仓库中基于 cargo 的项目](#个人仓库中基于-cargo-的项目)
3. [组织仓库中基于 cargo 的项目](#组织仓库中基于-cargo-的项目)
4. [发布到 crates.io 上的 crate](#发布到-cratesio-上的-crate)

### git 子模块

最特殊的依赖方式，对于那些不使用 cargo（由于不是 rust 编写）的项目不得不如此。

目前使用此方式依赖的项目包括：

- libc-test

  musl libc 的测例。使用 c 语言实现。

- tests

  测试框架。主要使用 python。

- rboot

  uefi 引导程序，使用 rust 实现。作为子模块是一个遗留问题，未来会解决。

### 个人仓库中基于 cargo 的项目

比较特殊的依赖方式，当且仅当一个项目具有下列情况之一：

- 属于实验性质，不稳定甚至可能放弃；
- 稳定，但不适合发布到 crates.io，也没有组织愿意收录；
- 从组织项目分叉，已提交 PR 但尚未采纳；
- 从组织项目分叉，且出于充分的理由决定永久分叉，成为一个独立的项目；

可以直接从个人项目中直接依赖。

如有可能，上述情况应当积极解决。

必须锁定提交哈希。

### 组织仓库中基于 cargo 的项目

正常的依赖方式，但若有可能，尽量发布到 crates.io。

必须锁定提交哈希。

### 发布到 crates.io 上的 crate

正常的依赖方式，可以随意使用。

尽量跟踪最新版本。对于无法跟踪的依赖，应记录理由。

## features and cfg

zCore 支持多种平台的不同硬件，难以避免使用编译选项。但是不当使用编译选项，可能导致混乱，或者干扰测试的覆盖性。因此，建议遵从如下标准设置 `#[cfg(...)]`：

1. 仅与由平台决定的，设置 target_arch；
2. 受到其他因素影响的，考虑通过设备树等方式动态决定；
3. 无法动态决定的，可以设置 feature，但应在所在项目的入口（lib.rs 或 main.rs）注明设置 feature 的原因；
4. 平台相关的 feature，应增加如下的约束，而不是使用 `all(target_arch = ..., feature = ...)`：

   ```rust
   #[cfg(all(feature = "sbi", not(target_arch = "riscv64")))]
   compile_error!("`sbi` is only available on RISC-V platforms");
   ```

> 现有代码不完全满足以上标准，将逐步改正
