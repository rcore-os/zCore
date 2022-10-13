﻿# zCore

[![CI](https://github.com/rcore-os/zCore/actions/workflows/build.yml/badge.svg?branch=master)](https://github.com/rcore-os/zCore/actions)
[![Docs](https://img.shields.io/badge/docs-pages-green)](https://rcore-os.github.io/zCore/)
[![Coverage Status](https://coveralls.io/repos/github/rcore-os/zCore/badge.svg?branch=master)](https://coveralls.io/github/rcore-os/zCore?branch=master)
[![issue](https://img.shields.io/github/issues/rcore-os/zCore)](https://github.com/rcore-os/zCore/issues)
[![forks](https://img.shields.io/github/forks/rcore-os/zCore)](https://github.com/rcore-os/zCore/fork)
![stars](https://img.shields.io/github/stars/rcore-os/zCore)
![license](https://img.shields.io/github/license/rcore-os/zCore)

An OS kernel based on zircon, provides Linux compatible mode.

- [中文自述文档](../README.md)
- [legacy README](README_LEGACY.md)

  > you may want to check the legacy for setting up docker, running graphical applications, etc. But many of these scripts are deprecated

## Launch zCore

   ```bash
   cargo qemu --arch riscv64
   ```

   This command will launch zCore using qemu-system-riscv64。

   The default file system will contain a busybox application and a musl-libc linker. They are compiled by automatic downloaded musl-libc RISC-V cross-compilation tool chain.

## Table of contents

- [Launch zCore](#launch-zcore)
- [Build the project](#build-the-project)
  - [Commands](#commands)
  - [Commands reference](#commands-reference)
- [Platform support](#platform-support)
  - [Qemu/virt](#qemuvirt)
  - [Allwinner/nezha](#allwinnernezha)
  - [starfivetech/visionfive](#starfivetechvisionfive)
  - [cvitek/cr1825](#cvitekcr1825)

## Build the project

The project will be built with [xtask](https://github.com/matklad/cargo-xtask). The common operations are provided as cargo commands.

An extra [Makefile](../Makefile) provides make calls for compatibility with some legacy scripts.

Currently tested development environments include Ubuntu 20.04, Ubuntu 22.04 and Debian 11.
The libc tests for x86_64 cannot compile on Ubuntu22.04.
If you do not need to flash to physical hardware, using WSL2 or other virtual machines does not operate any differently from the real machine.

### Commands

The basic format of the command is `cargo <command> [--rags [value]]`, which is actually `cargo run --package xtask --release -- <command> [--args [value]]`. `command` is passed to the xtask application to parse and execute.

The effects of many commands are affected by the repo environment and will also affect the repo environment. For convenience, if one command depends on the result of another command, they are designed to recursion. The recursive relationship diagram of the commands is as follows. The detailed explanation of them is in the next section:

---

> **NOTICE** It is recommended to use equivalent fonts

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
Example：`A` recursively executing `B` (`A` depends on the results of `B`, and `B` is executed before `A` automatically)
┌───┐  ┌───┐
| A |─→| B |
└───┘  └───┘
```

### Commands reference

If the following command description does not match its behavior, or if you suspect that this documentation is not up to date, you can check the [inline documentation](../xtask/src/main.rs#L48) as well.
If you find `error: no such subcommand: ...`, check [command alias](../.cargo/config.toml) to see which commands have aliases set for them.

---

> **NOTICE** inline documentation is also bilingual

---

#### **update-all**

Updates toolchain、dependencies and submodules.

```bash
cargo update-all
```

#### **check-style**

Checks code without running. Try to compile the project with various different features.

```bash
cargo check-style
```

#### **zircon-init**

Download zircon binaries.

```bash
cargo zircon-init
```

#### **asm**

Dumps the asm of kernel for specific architecture.
The default output is `target/zcore.asm`.

```bash
cargo asm -m virt-riscv64 -o z.asm
```

#### **bin**

Strips kernel binary for specific architecture.
The default output is `target/{arch}/release/zcore.bin`.

```bash
cargo bin -m virt-riscv64 -o z.bin
```

#### **qemu**

Runs zCore in qemu.

```bash
cargo qemu --arch riscv64 --smp 4
```

Connects qemu to gdb：

```bash
cargo qemu --arch riscv64 --smp 4 --gdb 1234
```

#### **rootfs**

Rebuilds the linux rootfs.
This command will remove the existing rootfs directory for this architecture,
and rebuild a minimum rootfs.

```bash
cargo rootfs --arch riscv64
```

#### **musl-libs**

Copies musl so files to rootfs directory.

```bash
cargo musl-libs --arch riscv64
```

#### **ffmpeg**

Copies ffmpeg so files to rootfs directory.

```bash
cargo ffmpeg --arch riscv64
```

#### **opencv**

Copies opencv so files to rootfs directory.
If ffmpeg is already there, this opencv will build with ffmpeg support.

```bash
cargo opencv --arch riscv64
```

#### **libc-test**

Copies libc test files to rootfs directory.

```bash
cargo libc-test --arch riscv64
```

#### **other-test**

Copies other test files to rootfs directory.

```bash
cargo other-test --arch riscv64
```

#### **image**

Builds the linux rootfs image file.

```bash
cargo image --arch riscv64
```

#### **linux-libos**

Runs zCore in linux libos mode and runs an executable at the specified path.

> **NOTICE** zCore can only run a single executable in libos mode, and it will exit after finishing.

```bash
cargo linux-libos --args /bin/busybox
```

## Platform support

### Qemu/virt

Launch with command directly, see [launch zCore](#launch-zcore).

### Allwinner/nezha

Build kernel binary with the following command:

```bash
cargo bin -m nezha -o z.bin
```

Then deploy the binary to Flash or DRAM with [rustsbi-d1](https://github.com/rustsbi/rustsbi-d1).

### Starfivetech/visionfive

Build kernel binary with the following command:

```bash
cargo bin -m visionfive -o z.bin
```

Then, see [this document](docs/README-visionfive.md) for detailed description, launching the system through u-boot network.

### cvitek/cr1825

Build kernel binary with the following command:

```bash
cargo bin -m cr1825 -o z.bin
```

Then launch the system through u-boot network.
