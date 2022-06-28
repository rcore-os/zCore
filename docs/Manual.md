# zCore 项目使用指南

项目构建采用 [xtask 模式](https://github.com/matklad/cargo-xtask)，主要操作封装为 cargo 命令，再通过 [Makefile](../Makefile) 提供 Legacy 调用，以兼容一些旧脚本。

## 常规操作流程

对于一般开发者和用户，可以按以下步骤设置 zCore 项目。

1. 先决条件

   目前已测试的开发环境包括 Ubuntu20.04、Ubuntu22.04 和 Debian11，
   Ubuntu22.04 不能正确编译 x86_64 的 libc 测试。
   若不需要烧写到物理硬件，使用 WSL2 或其他虚拟机的操作与真机并无不同之处。

   在开始之前，确保你的计算机上安装了 git、git lfs 和 rustup。要在虚拟环境开发或测试，需要 QEMU。

2. 克隆项目

   ```bash
   git clone https://github.com/rcore-os/zCore.git
   ```

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

如果命令描述与行为不符，或怀疑此文档更新不及时，亦可直接查看 [内联文档](../xtask/src/main.rs#L48)。
如果发现 `error: no such subcommand: ...`，查看 [命令简写](../.cargo/config.toml) 为哪些命令设置了别名。

### 常用功能

- **dump**

打印构建信息。Dumps build config.

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

内核反汇编。将适应指定架构的内核反汇编并输出到文件。默认输出文件为项目目录下的 `zcore.asm`。

```bash
cargo asm --arch riscv64 --output riscv64.asm
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
