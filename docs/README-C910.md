# zCore on riscv64

## T-HEAD C910 Light val board 操作说明
### 编译zCore内核镜像
编译zCore内核:
```
cd zCore/zCore
make build LINUX=1 MODE=release ARCH=riscv64 PLATFORM=c910light
```

制作u-boot系统镜像:
```
mkimage -A riscv -O linux -C none -T kernel -a 0x200000 -e 0x200000 -n "zCore for c910" -d ../target/riscv64/release/zcore.bin uImageC910
```

### 编译opensbi镜像
```
git clone https://github.com/elliott10/opensbi.git -b thead_light-c910

cd opensbi

make PLATFORM=generic CROSS_COMPILE=/path/to/toolchain/bin/riscv64-unknown-linux-gnu-
# 生成所需的fw_dynamic.bin
```
注：原编译工具链基于官方仓库https://gitee.com/thead-yocto/xuantie-yocto.git 编译生成出来的。理论上可以使用其他工具链替代之

### 基于u-boot运行

在搭建好tftp服务的服务器目录中，放入编译好的opensbi镜像`fw_dynamic.bin`和系统镜像`uImageC910`。<br>
进入配置好网络的C910 Light板子的u-boot命令行上，运行：
```
ext4load mmc 0:2 $aon_ram_addr light_aon_fpga.bin; ext4load mmc 0:2 $dtb_addr ${fdt_file};

tftp $opensbi_addr fw_dynamic.bin;
tftp $kernel_addr uImageC910;

bootslave; run finduuid; run set_bootargs; bootm $kernel_addr - $dtb_addr;

```

## T-HEAD C910 Light val board 移植说明
### 系统初步分析并制作系统镜像

(板子图)

在取得C910 Light开发板后，先按照官方的用户手册了解基本的硬件组件，以及电源和串口等接口的接线。<br>
用户手册：https://gitee.com/thead-yocto/documents/blob/master/en/user_guide/T-Head%20Yeying1520%20Yocto%20User%20Guide.pdf <br>

电源接上，以及串口连接到host机后，可以看到有四个串口`/dev/ttyUSBX`，调试用串口是，host机连上该串口，并在启动u-boot时按任意键可进入命令行模式<br>
u-boot命令行模式上，可以连接通有线网络，并通过tftp协议加载待启动的操作系统镜像。<br>
```
# minicom -b 115200 -D /dev/ttyUSB2

U-Boot 2020.01-ge0ddd4721a (Dec 14 2021 - 22:26:59 +0800)

CPU:   rv64imafdcvsu
Model: T-HEAD c910 light
DRAM:  1 GiB
C910 CPU FREQ: 1500MHz
AHB2_CPUSYS_HCLK FREQ: 250MHz
AHB3_CPUSYS_PCLK FREQ: 125MHz
PERISYS_AHB_HCLK FREQ: 250MHz
PERISYS_APB_PCLK FREQ: 62MHz
GMAC PLL POSTDIV FREQ: 1000MHZ
DPU0 PLL POSTDIV FREQ: 1188MHZ
DPU1 PLL POSTDIV FREQ: 1188MHZ
MMC:   sdhci@ffe7080000: 0, sd@ffe7090000: 1
Loading Environment from MMC... OK
In:    serial@ffe7014000
Out:   serial@ffe7014000
Err:   serial@ffe7014000
Net:   eth0: ethernet@ffe7070000
Hit any key to stop autoboot:  0 
C910 Light# 

# setenv ipaddr <IP>
# setenv serverip <Server IP>

```

在这里一开始尝试fu740板子的zCore系统镜像制作以及tftp启动模式, 在u-boot命令行中配置开发板IP地址和服务器IP地址，通过tftp协议加载系统镜像，最后通过bootm运行镜像; <br>

其他正常，但这样在引导zCore系统镜像时会出现报错，显示无法识别该系统镜像.
```
Wrong Image Format for bootm command
ERROR: can't get kernel image!
```
这个问题分析后，发现由于u-boot的版本不同，导致对系统镜像格式的识别不一致。<br>
在fu740板子上的zCore系统镜像是基于`.its`脚本的新`FIT image`;<br>
而C910 Light的板子上的u-boot则只支持`old legacy image`，需要这样制作：
```
mkimage -A riscv -O linux -C none -T kernel -a 0x200000 -e 0x200000 -n "zCore for c910" -d ../target/riscv64/release/zcore.bin uImageC910

```
### zCore系统引导构建


###
