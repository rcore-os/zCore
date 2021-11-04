# zCore for riscv64

## 编译zCore系统镜像

先在源码根目录下编译riscv64的文件系统。<br>
然后进入子目录zCore编译内核,会生成系统镜像`target/riscv64/debug/zcore.bin`

```
make riscv-image
cd zCore
make build LINUX=1 ARCH=riscv64 PLATFORM=d1

```

## riscv64开发板的烧写
以全志D1 c906开发板为例.<br>

下载并编译烧写工具xfel:
```
git clone https://github.com/xboot/xfel.git
cd xfel
make
```

生成包含了opensbi和zCore的待烧写固件:
```
cp ../prebuilt/firmware/fw_jump-0x40020000.bin fw-zCore.bin
dd if=target/riscv64/debug/zcore.bin of=fw-zCore.bin bs=1 seek=131072
```

启动全志D1 c906开发板，并进入FEL模式。可在开发板的Linux系统中执行`reboot efex`命令进入FEL模式。<br>
然后通过烧写工具xfel把zCore系统镜像载入到DDR中，并运行：
```
sudo xfel ddr ddr3
sudo xfel write 0x40000000 fw-zCore.bin
sudo xfel exec 0x40000000
```

或者在安装好工具xfel，开发板进入FEL模式后，直接运行：
```
make run-thead LINUX=1 ARCH=riscv64 PLATFORM=d1
```

## 引导运行

zCore成功引导后将如下所示：
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
