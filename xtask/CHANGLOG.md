# 更新公告

最新的更新将出现在最上方。

## 20220511(YdrMaster)

- 现在用于 linux 模式的 rootfs 统一放在 rootfs/$ARCH，未来新增 aarch64 或更多架构也将放在这个目录；
- libc-test 现在是一个子模块，不再需要单独 git clone；
- 使用 `std::os::unix::fs::symlink` 建立符号链接，不再依赖 `ln` 应用程序；
