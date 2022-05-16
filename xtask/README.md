# zCore tasks

[更新公告](CHANGLOG.md)

## 使用说明

使用 `cargo xtask help` 将打印出命令说明。对主要子命令的简要说明如下：

1. `git-proxy`: 设置 git 代理；
2. `setup`: 拉取预编译文件和子项目，补全仓库；
3. `update-all`: 更新工具链、依赖和子项目；
4. `check-style`: 编译前检查；
5. `rootfs --arch <arch>`: 创建架构相关的 linux 启动文件系统；
6. `libc-test --arch <arch>`: 向启动文件系统加入 libc 测试；
7. `other-test --arch <arch>`: 向启动文件系统加入其他测试；
8. `image --arch <arch>`: 打包指定架构的系统镜像；
9. `asm --arch <arch> --output <file name>`: 构建指定架构内核，并反汇编到文件；
10. `qemu --arch <arch>`: 启动指定架构的 Qemu 运行内核；
11. `gdb --arch <arch> --port <port>`: 启动指定架构的 GDB 并连接到端口；

其中 6、7、8 执行时若 5 的结果不存在，将自动递归执行 5；

## 项目愿景

这个任务系统用于辅助 zCore 的开发、使用和测试工作。

为了实现这个目标，需要提供三类命令：

- [ ] 构造开发环境
  - [x] 更新子项目/子仓库
  - [x] 更新工具链/依赖
  - [ ] 下载并编译 Qemu
  - [ ] 下载并编译 GDB
  - [x] 更新测例和测试系统
- [ ] 支持 zCore 使用
  - [ ] 以 LibOS 形式启动 zCore
  - [x] 在 Qemu 环境中启动 zCore
  - [x] 打包可烧写到其他存储介质的 zCore 镜像
  - [x] 启动与 Qemu 关联的 GDB 以支持内核调试
- [ ] 自动化测试：实现与 CI 的逻辑一致性
