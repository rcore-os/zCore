# zCore

[![CI](https://github.com/rcore-os/zCore/actions/workflows/build.yml/badge.svg?branch=master)](https://github.com/rcore-os/zCore/actions)
[![Coverage Status](https://coveralls.io/repos/github/rcore-os/zCore/badge.svg?branch=master)](https://coveralls.io/github/rcore-os/zCore?branch=master)
[![issue](https://img.shields.io/github/issues/rcore-os/zCore)](https://github.com/rcore-os/zCore/issues)
[![forks](https://img.shields.io/github/forks/rcore-os/zCore)](https://github.com/rcore-os/zCore/fork)
![stars](https://img.shields.io/github/stars/rcore-os/zCore)
![license](https://img.shields.io/github/license/rcore-os/zCore)

An OS kernel based on zircon, provides Linux compatible mode.

- [中文自述文档](../README.md)
- [legacy README](README_LEGACY.md)
  > you may want to check the legacy for setting up docker, running graphical applications, etc. But many of these scripts are deprecated
- [builder change log](../xtask/CHANGELOG.md)

## Build the project

The project should be built with [xtask](https://github.com/matklad/cargo-xtask). The common operations are provided as cargo commands. An extra [Makefile](../Makefile) provides make calls for compatibility with some legacy scripts.

Developers and users may set up the project by following the steps below:

1. Environment

   Currently tested development environments include Ubuntu 20.04, Ubuntu 22.04 and Debian 11.
   The libc tests for x86_64 cannot compile on Ubuntu22.04.
   If you do not need to flash to physical hardware, using WSL2 or other virtual machines does not operate any differently from the real machine.

   Make sure you have git, git lfs, and rustup installed on your computer before you start. Qemu is required to develop or test in a virtual environment.

2. clone the repo

   ```bash
   git clone https://github.com/rcore-os/zCore.git
   ```

   > **NOTICE** It's not necessary to recurse here, as it will will automatically pull the submodules at the next step

3. initialize the local repo

   ```bash
   cargo initialize
   ```

4. keep up to date

   ```bash
   cargo update-all
   ```

5. need help?

   ```bash
   cargo xtask
   ```

## Command reference

If the following command description does not match its behavior, or if you suspect that this documentation is not up to date, you can check the [inline documentation](../xtask/src/main.rs#L48) as well.
If you find `error: no such subcommand: ... `, check [command alias](../.cargo/config.toml) to see which commands have aliases set for them.

> **NOTICE** inline documentation is also bilingual

### Common functions

- **dump**

Dumps build config.

```bash
cargo dump
```

### Project and local repo

- **initialize**

Initializes the project. Git lfs install and pull. Submodules will be updated.

```bash
cargo initialize
```

- **update-all**

Updates toolchain、dependencies and submodules.

```bash
cargo update-all
```

- **check-style**

Checks code without running. Try to compile the project with various different features.

```bash
cargo check-style
```

### Develop and Debug

- **asm**

Dumps the asm of kernel for specific architecture.
The default output is `zcore.asm` in the project directory.

```bash
cargo asm --arch riscv64 --output riscv64.asm
```

- **qemu**

Runs zCore in qemu.

```bash
cargo qemu --arch riscv64 --smp 4
```

Connects qemu to gdb：

```bash
cargo qemu --arch riscv64 --smp 4 --gdb 1234
```

- **gdb**

Launches gdb and connects to a port.

```bash
cargo gdb --arch riscv64 --port 1234
```

### manage linux rootfs

- **rootfs**

Rebuilds the linux rootfs.
This command will remove the existing rootfs directory for this architecture,
and rebuild a minimum rootfs.

```bash
cargo rootfs --arch riscv64
```

- **musl-libs**

Copies musl so files to rootfs directory.

```bash
cargo musl-libs --arch riscv64
```

- **ffmpeg**

Copies ffmpeg so files to rootfs directory.

```bash
cargo ffmpeg --arch riscv64
```

- **opencv**

Copies opencv so files to rootfs directory.
If ffmpeg is already there, this opencv will build with ffmpeg support.

```bash
cargo opencv --arch riscv64
```

- **libc-test**

Copies libc test files to rootfs directory.

```bash
cargo libc-test --arch riscv64
```

- **other-test**

Copies other test files to rootfs directory.

```bash
cargo other-test --arch riscv64
```

- **image**

Builds the linux rootfs image file.

```bash
cargo image --arch riscv64
```

### Libos mode

- **linux-libos**

Runs zCore in linux libos mode and runs an executable at the specified path.

> **NOTICE** zCore can only run a single executable in libos mode, and it will exit after finishing.

```bash
cargo linux-libos --args /bin/busybox
```
