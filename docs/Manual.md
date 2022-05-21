# zCore 项目使用指南

## 预定功能

预定功能指的是 zCore 作为一个项目，为开发者和用户常用操作提供的封装。

由于历史原因，目前预定功能分为顶层预定功能和内核预定功能。
所有顶层提供的预定功能都定义于 [顶层 Makefile](../Makefile)，
并且所有预定功能最终都将移动到顶层。

## 常规操作流程

对于一般开发者和用户，可以按以下步骤设置 zCore 项目。

1. 先决条件

   目前已测试的开发环境包括 Ubuntu20.04、Ubuntu22.04 和 Debian11，
   Ubuntu22.04 不能正确编译 x86_64 的 libc 测试。
   若不需要烧写到物理硬件，使用 WSL2 或其他虚拟机的操作与真机并无不同之处。

   在开始之前，确保你的计算机上安装了 git 和 rustup。要在虚拟环境开发或测试，需要 QEMU。

2. 克隆项目

   ```bash
   git clone https://github.com/rcore-os/zCore.git
   ```

3. 初始化存储库

   ```bash
   make setup
   ```

4. 保持更新

   ```bash
   make update
   ```

5. 探索更多操作

   ```bash
   make help
   ```

6. 推到仓库前，现在本机执行测试

   ```bash
   make check # CI/build 的一部分，未来会实现更多快速测试指令
   ```

## Linux 模式

zCore 根据向用户提供的系统调用的不同，可分为 zircon 模式和 linux 模式。
要以 linux 模式启动，需要先构建 linux 的启动文件系统。

这个指令构建适于 x86_64 架构的启动文件系统。

```bash
make rootfs ARCH=x86_64
```

这个指令构建适于 riscv64 架构的启动文件系统。

```bash
make rootfs ARCH=riscv64
```

要执行 musl-libc 测试集，需要向文件系统中添加 libc 测试集：

```bash
make libc-test <ARCH=?>
```

要执行 CI 的其他测试，需要向文件系统中添加相应测试集：

```bash
make other-test <ARCH=?>
```

要以裸机模式启动 zCore，需要构造将放到设备或虚拟环境中的镜像文件：

```bash
make image <ARCH=?>
```
