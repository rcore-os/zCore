# zCore on riscv64 for fu740

### 编译zCore for fu740系统镜像

先在源码根目录下编译 riscv64 的文件系统。

然后进入子目录 zCore 编译内核,会生成系统镜像`zcore-fu740.itb`

系统镜像会包含fu740板子的设备树, dtb设备树可以从fu740自带的Linux中的`/boot`目录中获取;
也可以从sifive官方镜像中获取：https://github.com/sifive/freedom-u-sdk/releases/download/2022.04.00/demo-coreip-cli-unmatched-2022.04.00.rootfs.wic.xz

```sh
make riscv-image
cd zCore
make build MODE=release LINUX=1 ARCH=riscv64 PLATFORM=fu740
```

### 基于U-Boot启动系统镜像

首先搭建一台tftp服务器, 例如，在Linux服务器中安装`tftpd-hpa`, 一般tftp服务的目录会在`/srv/tftp/`;

然后把编译出的zCore for fu740系统镜像`zcore-fu740.itb`拷贝到tftp服务目录；

开发板fu740开机，并进入U-Boot命令行：

```
# 配置开发板IP地址和服务器IP地址
setenv ipaddr <IP>
setenv serverip <Server IP>

# 通过tftp协议加载系统镜像
tftp 0xa0000000 zcore-fu740.itb

# 运行
bootm 0xa0000000
```

### fu740资料汇集
Unmatched fu740 板子资料整理， 请见：
https://github.com/oscomp/DocUnmatchedU740/blob/main/unmatched.md
