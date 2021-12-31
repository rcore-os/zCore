# zCore for riscv64

## 编译 zCore 系统镜像

先在源码根目录下编译 riscv64 的文件系统。

然后进入子目录 zCore 编译内核,会生成系统镜像`zcore.bin`

```sh
make riscv-image
cd zCore
make build LINUX=1 ARCH=riscv64 PLATFORM=d1 MODE=release
```

## riscv64 开发板的烧写

以全志 D1 c906 开发板为例。

下载并编译烧写工具 `xfel`:

```sh
git clone https://github.com/xboot/xfel.git
cd xfel
make
```

### 自动烧写运行：

安装好工具 `xfel`，开发板进入 FEL 模式，可在开发板的 Linux 系统中执行 `reboot efex` 命令进入 FEL 模式。然后运行：

```sh
make run_d1 LINUX=1 ARCH=riscv64 PLATFORM=d1 MODE=release
```

### 手动烧写运行：

1. 下载 D1 开发板的 [OpenSBI](https://github.com/elliott10/opensbi) 源码，并编译出镜像 build/platform/thead/c910/firmware/fw_payload.elf：

    ```sh
    git clone https://github.com/elliott10/opensbi -b thead
    cd opensbi
    make PLATFORM=thead/c910 CROSS_COMPILE=/path/to/toolchain/bin/riscv64-unknown-linux-gnu- SUNXI_CHIP=sun20iw1p1 PLATFORM_RISCV_ISA=rv64gcxthead
    ```

    或使用预编译的镜像 [prebuilt/firmware/d1/fw_payload.elf](../prebuilt/firmware/d1/fw_payload.elf)。

2. 生成包含了 OpenSBI, dtb, zCore 的待烧写固件:

    ```sh
    rust-objcopy --binary-architecture=riscv64 ../prebuilt/firmware/d1/fw_payload.elf --strip-all -O binary ./zcore_d1.bin
    dd if=../target/riscv64/release/zcore.bin of=zcore_d1.bin bs=512 seek=2048
    ```

3. 启动全志 D1 c906 开发板，并进入 FEL 模式。然后通过烧写工具 `xfel` 把 zCore 系统镜像载入到 DDR 中：

    ```
    sudo xfel ddr ddr3
    sudo xfel write 0x40000000 zcore_d1.bin
    sudo xfel exec 0x40000000
    ```

## 引导运行

zCore 成功引导后, OpenSBI 会将 dtb 加载到高地址 `0x5ff00000`，运行如下所示：

```
OpenSBI smartx-d1-tina-v1.0.1-release
   ____                    _____ ____ _____
  / __ \                  / ____|  _ \_   _|
 | |  | |_ __   ___ _ __ | (___ | |_) || |
 | |  | | '_ \ / _ \ '_ \ \___ \|  _ < | |
 | |__| | |_) |  __/ | | |____) | |_) || |_
  \____/| .__/ \___|_| |_|_____/|____/_____|
        | |
        |_|

Platform Name          : T-HEAD Xuantie Platform
Platform HART Features : RV64ACDFIMSUVX
Platform Max HARTs     : 1
Current Hart           : 0
Firmware Base          : 0x40000400
Firmware Size          : 75 KB
Runtime SBI Version    : 0.2

MIDELEG : 0x0000000000000222
MEDELEG : 0x000000000000b1ff
PMP0    : 0x0000000040000000-0x000000004001ffff (A)
PMP1    : 0x0000000040000000-0x000000007fffffff (A,R,W,X)
PMP2    : 0x0000000080000000-0x00000000bfffffff (A,R,W,X)
PMP3    : 0x0000000000020000-0x0000000000027fff (A,R,W,X)
PMP4    : 0x0000000000000000-0x000000003fffffff (A,R,W)
      ____
 ____/ ___|___  _ __ ___
|_  / |   / _ \| '__/ _ \
 / /| |__| (_) | | |  __/
/___|\____\___/|_|  \___|

Welcome to zCore rust_main( hartid: 0x0, device_tree_paddr: 0x44ddc )
Uart output testing
+++ Setting up UART interrupts +++
+++ Setting up PLIC +++
+++ setup interrupt +++
Exception::Breakpoint: A breakpoint set @0xffffffffc0167f56
Device Tree @ 0x0
[138.8430296s  WARN 0 0:0] elf relocate Err:".rela.dyn not found"
[139.2137079s  WARN 0 0:0] brk: unimplemented
[139.8335662s  WARN 0 0:0] TCGETS | TIOCGWINSZ | TIOCSPGRP, pretend to be tty.
[140.5358217s  WARN 0 0:0] TIOCGPGRP, pretend to be have a tty process group.
[140.6017734s  WARN 0 0:0] getpgid: unimplemented
[140.8971624s  WARN 0 0:0] setpgid: unimplemented
/ #
/ # ls
bin  dev  tmp
/ # hello
Hello world from user mode program!
                                   By xiaoluoyuan@163.com
/ #

```
