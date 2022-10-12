# zCore

[![CI](https://github.com/rcore-os/zCore/actions/workflows/build.yml/badge.svg?branch=master)](https://github.com/rcore-os/zCore/actions)
[![Docs](https://img.shields.io/badge/docs-pages-green)](https://rcore-os.github.io/zCore/)
[![Coverage Status](https://coveralls.io/repos/github/rcore-os/zCore/badge.svg?branch=master)](https://coveralls.io/github/rcore-os/zCore?branch=master)
[![issue](https://img.shields.io/github/issues/rcore-os/zCore)](https://github.com/rcore-os/zCore/issues)
[![forks](https://img.shields.io/github/forks/rcore-os/zCore)](https://github.com/rcore-os/zCore/fork)
![stars](https://img.shields.io/github/stars/rcore-os/zCore)
![license](https://img.shields.io/github/license/rcore-os/zCore)

基于 zircon 并提供 Linux 兼容性的操作系统内核。

## 原版README

  Reimplement `Zircon` microkernel in safe Rust as a userspace program!

- zCore设计架构概述
- 支持bare-metal模式的Zircon & Linux
- 支持libos模式的Zircon & Linux
- 支持的图形应用程序等更多指导请查看[原版README文档](README-arch.md)。

## 启动内核

   ```bash
   cargo qemu --arch riscv64
   ```

   这个命令会使用 qemu-system-riscv64 启动 zCore。

   默认的文件系统中将包含 busybox 应用程序和 musl-libc 链接器。它们是用自动下载的 musl-libc RISC-V 交叉编译工具链编译的。

## 目录

- [启动内核](#启动内核)
- [项目构建](#项目构建)
  - [构建命令](#构建命令)
  - [命令参考](#命令参考)
- [平台支持](#平台支持)
  - [Qemu/virt](#qemuvirt)
  - [全志/哪吒](#全志哪吒)
  - [赛昉/星光](#赛昉星光)
  - [晶视/cr1825](#晶视cr1825)

## 项目构建

项目构建采用 [xtask 模式](https://github.com/matklad/cargo-xtask)，常用操作被封装成 cargo 命令。

另外，还通过 [Makefile](Makefile) 提供 make 调用，以兼容一些旧脚本。

目前已测试的开发环境包括 Ubuntu20.04、Ubuntu22.04 和 Debian11，Ubuntu22.04 不能正确编译 x86_64 的 libc 测试。若不需要烧写到物理硬件，使用 WSL2 或其他虚拟机的操作与真机并无不同之处。

### 构建命令

命令的基本格式为 `cargo <command> [--args [value]]`，这实际上是 `cargo run --package xtask --release -- <command> [--args [value]]` 的简写。`command` 被传递给 xtask 应用程序，解析并执行。

许多命令的效果受到仓库环境的影响，也会影响仓库的环境。为了使用方便，如果一个命令依赖于另一个命令的效果，它们被设计为递归的。命令的递归关系图如下，对于它们的详细解释在下一节：

---

> **NOTICE** 建议使用等宽字体

---

```text
┌────────────┐ ┌─────────────┐ ┌─────────────┐
| update-all | | check-style | | zircon-init |
└────────────┘ └─────────────┘ └─────────────┘
┌─────┐ ┌──────┐  ┌─────┐  ┌─────────────┐ ┌─────────────────┐
| asm | | qemu |─→| bin |  | linux-libos | | libos-libc-test |
└─────┘ └──────┘  └─────┘  └─────────────┘ └─────────────────┘
                     |            └───┐┌─────┘   ┌───────────┐
                     ↓                ↓↓      ┌──| libc-test |
                 ┌───────┐        ┌────────┐←─┘  └───────────┘
                 | image |───────→| rootfs |←─┐ ┌────────────┐
                 └───────┘        └────────┘  └─| other-test |
                 ┌────────┐           ↑         └────────────┘
                 | opencv |────→┌───────────┐
                 └────────┘  ┌─→| musl-libc |
                 ┌────────┐  |  └───────────┘
                 | ffmpeg |──┘
                 └────────┘
-------------------------------------------------------------------
图例：A 递归执行 B（A 依赖 B 的结果，执行 A 时自动先执行 B）
┌───┐  ┌───┐
| A |─→| B |
└───┘  └───┘
```

### 命令参考

如果下面的命令描述与行为不符，或怀疑此文档更新不及时，亦可直接查看[内联文档](xtask/src/main.rs#L48)。
如果发现 `error: no such subcommand: ...`，查看[命令简写](.cargo/config.toml)为哪些命令设置了别名。

---

> **NOTICE** 内联文档也是中英双语

---

#### **update-all**

更新工具链、依赖和 git 子模块。

如果没有递归克隆子模块，可以使用这个命令克隆。

```bash
cargo update-all
```

#### **check-style**

静态检查。设置多种编译选项，检查代码能否编译。

```bash
cargo check-style
```

#### **zircon-init**

下载 zircon 模式所需的二进制文件。

```bash
cargo zircon-init
```

#### **asm**

反汇并保存编指定架构的内核。默认保存到 `target/zcore.asm`。

```bash
cargo asm -m virt-riscv64 -o z.asm
```

#### **bin**

生成内核 raw 镜像到指定位置。默认输出到 `target/{arch}/release/zcore.bin`。

```bash
cargo bin -m virt-riscv64 -o z.bin
```

#### **qemu**

在 Qemu 中启动 zCore。这需要 Qemu 已经安装好了。

```bash
cargo qemu --arch riscv64 --smp 4
```

支持将 qemu 连接到 gdb：

```bash
cargo qemu --arch riscv64 --smp 4 --gdb 1234
```

#### **rootfs**

重建 Linux rootfs。直接执行这个命令会清空已有的为此架构构造的 rootfs 目录，重建最小的 rootfs。

```bash
cargo rootfs --arch riscv64
```

#### **musl-libs**

将 musl 动态库拷贝到 rootfs 目录对应位置。

```bash
cargo musl-libs --arch riscv64
```

#### **ffmpeg**

将 ffmpeg 动态库拷贝到 rootfs 目录对应位置。

```bash
cargo ffmpeg --arch riscv64
```

#### **opencv**

将 opencv 动态库拷贝到 rootfs 目录对应位置。如果 ffmpeg 已经放好了，opencv 将会编译出包含 ffmepg 支持的版本。

```bash
cargo opencv --arch riscv64
```

#### **libc-test**

将 libc 测试集拷贝到 rootfs 目录对应位置。

```bash
cargo libc-test --arch riscv64
```

#### **other-test**

将其他测试集拷贝到 rootfs 目录对应位置。

```bash
cargo other-test --arch riscv64
```

#### **image**

从 rootfs 目录构建 Linux rootfs 镜像文件。

```bash
cargo image --arch riscv64
```

#### **linux-libos**

在 linux libos 模式下启动 zCore 并执行位于指定路径的应用程序。

> **NOTICE** libos 模式只能执行单个应用程序，完成就会退出。

```bash
cargo linux-libos --args "/bin/busybox"
```

可以直接给应用程序传参数：

```bash
cargo linux-libos --args "/bin/busybox ls"
```

## 平台支持

### Qemu/virt

直接使用命令启动，参见[启动内核](#启动内核)和 [`qemu` 命令](#qemu)。

### 全志/哪吒

使用以下命令构造系统镜像：

```bash
cargo bin -m nezha -o z.bin
```

然后使用 [rustsbi-d1](https://github.com/rustsbi/rustsbi-d1) 将镜像部署到 Flash 或 DRAM。

另: 可以查看[README for D1 文档](docs/README-D1.md)获知更多D1开发板有关的操作指导。

### 赛昉/星光

使用以下命令构造系统镜像：

```bash
cargo bin -m visionfive -o z.bin
```

然后根据[此文档](docs/README-visionfive.md)的详细说明通过 u-boot 网络启动系统。

### 晶视/cr1825

使用以下命令构造系统镜像：

```bash
cargo bin -m cr1825 -o z.bin
```

然后通过 u-boot 网络启动系统。

## 其他

- [An English README](docs/README_EN.md)
- [开发者注意事项（草案）](docs/for-developers.md)
- [构建系统更新日志](xtask/CHANGELOG.md)
