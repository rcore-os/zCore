# 更新公告

最新的更新将出现在最上方。

## 20220511(YdrMaster)

### 目录结构定义

- 现在用于 linux 模式的 rootfs 统一放在 `rootfs/{arch}` 目录，未来新增 aarch64 或更多架构也将放在这个目录；
- 不再将构建过程产生的东西放在 `prebuilt` 目录。现在 `prebuilt` 目录完全来自 `git lfs pull`；
- 下载的源文件放在 `ignored/origin/{arch}`，`make clean` 时不会清除这些文件；
- 解压或构建过程产生的其他文件放在 `ignored/target/{arch}`，`make clean` 会清除这些文件；
- `libc-test` 现在是一个子模块，不再需要单独 `git clone`；

### 使用步骤

- 现在 `cargo rootfs {arch}` 将清空已有 `rootfs/{arch}`，然后产生供 zCore 以 linux 模式启动的最小文件系统——只有 `/bin/busybox` 和 `lib/lib/ld-musl-{arch}.so.1` 两个文件，以及一些指向 busybox 的符号链接；
- `cargo libc-test {arch}` 命令将向 `rootfs/{arch}` 放入 libc 测试的测例文件，在必要时下载交叉编译工具链；
- 增加 `cargo other-test {arch}` 命令，向 `rootfs/{arch}` 放入其他测试的测例文件；
- `cargo image {arch}` 命令将 `rootfs/{arch}` 打包成 `zCore/{arch}.img` 文件，过程中不关心 `rootfs/{arch}` 的内容。因此如需要向文件系统加入文件，在 `image` 之前放入 `rootfs/{arch}` 即可；
- `libc-test`、`other-test`、`image` 都不需要先 `rootfs`，如果 `rootfs/{arch}` 目录不存在，将自动创建；

### 实现变更

- 使用 `std::os::unix::fs::symlink` 建立符号链接，不再依赖 `ln` 应用程序；
