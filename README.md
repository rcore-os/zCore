# zCore

[![CI](https://github.com/rcore-os/zCore/actions/workflows/build.yml/badge.svg?branch=master)](https://github.com/rcore-os/zCore/actions)
[![Docs](https://img.shields.io/badge/docs-pages-green)](https://rcore-os.github.io/zCore/)
[![Coverage Status](https://coveralls.io/repos/github/rcore-os/zCore/badge.svg?branch=master)](https://coveralls.io/github/rcore-os/zCore?branch=master)
[![issue](https://img.shields.io/github/issues/rcore-os/zCore)](https://github.com/rcore-os/zCore/issues)
[![forks](https://img.shields.io/github/forks/rcore-os/zCore)](https://github.com/rcore-os/zCore/fork)
![stars](https://img.shields.io/github/stars/rcore-os/zCore)
![license](https://img.shields.io/github/license/rcore-os/zCore)

基于 zircon 并提供 Linux 兼容操作系统内核。

- [An English README](docs/README_EN.md)
- [原版 README](docs/README_LEGACY.md)
  > 关于设置 docker、构建图形应用等操作可能需要查询原版 README，但其中很多脚本都废弃了
- [构建系统更新日志](xtask/CHANGELOG.md)
- [开发者注意事项（草案）](docs/for-developers.md)

## 构建项目

项目构建采用 [xtask 模式](https://github.com/matklad/cargo-xtask)，常用操作被封装成 cargo 命令，再通过 [Makefile](Makefile) 提供 make 调用，以兼容一些旧脚本。

开发者和用户可以按以下步骤设置 zCore 项目。

1. 先决条件

   目前已测试的开发环境包括 Ubuntu20.04、Ubuntu22.04 和 Debian11，
   Ubuntu22.04 不能正确编译 x86_64 的 libc 测试。
   若不需要烧写到物理硬件，使用 WSL2 或其他虚拟机的操作与真机并无不同之处。

   在开始之前，确保你的计算机上安装了 git、git lfs 和 rustup。要在虚拟环境开发或测试，需要 QEMU。

2. 克隆项目

   ```bash
   git clone https://github.com/rcore-os/zCore.git
   ```

   > **NOTICE** 此处不必递归，因为后续步骤会自动拉取子项目

3. 初始化存储库

   ```bash
   cargo initialize
   ```

4. 保持更新

   ```bash
   cargo update-all
   ```

5. 探索更多操作

   ```bash
   cargo xtask
   ```

## 命令参考指南

如果下面的命令描述与行为不符，或怀疑此文档更新不及时，亦可直接查看[内联文档](xtask/src/main.rs#L48)。
如果发现 `error: no such subcommand: ...`，查看[命令简写](.cargo/config.toml)为哪些命令设置了别名。

> **NOTICE** 内联文档也是中英双语

### 常用功能

- **dump**

打印构建信息。

```bash
cargo dump
```

### 项目构建和管理

- **initialize**

初始化项目。转换 git lfs 并更新子项目。

```bash
cargo initialize
```

- **update-all**

更新工具链、依赖和子项目。

```bash
cargo update-all
```

- **check-style**

静态检查。设置多种编译选项，检查代码能否编译。

```bash
cargo check-style
```

### 开发和调试

- **asm**

反汇并保存编指定架构的内核。默认保存到 `target/zcore.asm`。

```bash
cargo asm --arch riscv64 --output riscv64.asm
```

- **bin**

生成内核 raw 镜像到指定位置。默认输出到 `target/{arch}/release/zcore.bin`。

```bash
cargo bin --arch riscv64 --output zcore.bin
```

- **qemu**

在 qemu 中启动 zCore。这需要 qemu 已经安装好了。

```bash
cargo qemu --arch riscv64 --smp 4
```

支持将 qemu 连接到 gdb：

```bash
cargo qemu --arch riscv64 --smp 4 --gdb 1234
```

- **gdb**

启动 gdb 并连接到指定端口。

```bash
cargo gdb --arch riscv64 --port 1234
```

### 管理 linux rootfs

- **rootfs**

重建 Linux rootfs。这个命令会清除已有的为此架构构造的 rootfs 目录，重建最小的 rootfs。

```bash
cargo rootfs --arch riscv64
```

- **musl-libs**

将 musl 动态库拷贝到 rootfs 目录对应位置。

```bash
cargo musl-libs --arch riscv64
```

- **ffmpeg**

将 ffmpeg 动态库拷贝到 rootfs 目录对应位置。

```bash
cargo ffmpeg --arch riscv64
```

- **opencv**

将 opencv 动态库拷贝到 rootfs 目录对应位置。如果 ffmpeg 已经放好了，opencv 将会编译出包含 ffmepg 支持的版本。

```bash
cargo opencv --arch riscv64
```

- **libc-test**

将 libc 测试集拷贝到 rootfs 目录对应位置。

```bash
cargo libc-test --arch riscv64
```

- **other-test**

将其他测试集拷贝到 rootfs 目录对应位置。

```bash
cargo other-test --arch riscv64
```

- **image**

构造 Linux rootfs 镜像文件。

```bash
cargo image --arch riscv64
```

### Libos 模式

- **linux-libos**

在 linux libos 模式下启动 zCore 并执行位于指定路径的应用程序。

> **NOTICE** libos 模式只能执行单个应用程序，完成就会退出。

```bash
cargo linux-libos --args /bin/busybox
```
