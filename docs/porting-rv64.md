# Porting zCore to RiscV64 (Qemu)

### 2021-03-04
<br>之前的内存部分，使用的比较简单的页表映射大页的方式；
<br>在加载Qemu文件系统镜像到内存中解开时使用，就需要对虚拟内存、virtio-blk-device以及SimpleFileSystem完全初始化好；
<br>其中有些可以比较方便地调用crate库；
<br>对中间抽象层kernel-hal的内存操作相关函数的实现：
* PageTable 新建页表或获取页表;
* trait PageTableTrait，从虚拟地址到物理地址的映射、查询等;
* hal_frame_alloc物理帧实现；
* pmem_write/read物理地址访问;

<br>其中，把rCore中的可复用的页表构建PageTableImpl，实际映射的MemoryArea，移到zCore中；
<br>在底下bare层的函数变量暴露给启动部分使用，修复一些错误；
<br>从目前的运行结果看，应该是页表部分应该是切换好了；
```
[1.2317591s DEBUG 0 0:0] switch table 8000000000080220 -> 8000000000080a49
```

### zCore系统结构<br>
![](./structure.svg)

### riscv64初始移植路径<br>
按照：<br>
|Qemu riscv64|-|kernel-hal-bare|-|kernel-hal|-|zircon-object/ linux-object|-|linux-syscall|-|linux-loader|-|busybox|
|-|-|-|-|-|-|-|-|-|-|-|-|-|

### 实现思路

* 分析Makefile的结构，弄清楚包括编译和运行的命令执行流；
* 结合Qemu virt和opensbi以及编译出所需文件格式的kernel，也包括用户文件系统生成由x86_64到riscv64，这些命令过程写入Makefile；
* 分析Cargo.toml的结构，理解各个features和依赖库crates的关系，如有些是可选的；
<br><br>
* 创建关于riscv64的Rust target-spec-json编译目标规格描述.json
* 创建riscv64对应的链接脚本.ld以及启动入口汇编.asm
* 重要的修改部分在kernel-hal-bare，arch下添加riscv架构，包括硬件初始化和对opensbi接口调用的封装；
<br><br>

* 先定一个小目标，让OS跑起来打印初始一段字符
* 这时会有大量的编译错误提示，需要解决，Rust错误提示很详细，会建议如何来修正：
  - 报错可能来自多个方面，包括：riscv64在Cargo.toml中的依赖库，非必需的crates先关掉，添加架构相关所需要的；
  - kernel-hal-bare中缺失的待实现的接口，可见kernel-hal中的定义；参考了arch/x86_64的函数；
  - 与target_arch由x86_64移植到riscv64的cfg，其相关的函数或变量需在代码中补上；
  - 变量及函数的作用范围等需要注意
* Rust的条件编译cfg，`#[cfg(target_arch = "x86_64")]`也需要为riscv64实现一份；

* 要让OS能打印，要把串口输出初始化；
  - 有两种方式：一种是调用opensbi的打印接口，一种是MMIO的方式初始化串口输出；
  - 然后实现fmt::Write和宏println；
<br><br>


* 关于中断，可通过调用crate riscv，方便的进行指令和寄存器操作；
* 陷入trap填入riscv64上下文切换的汇编;
* 初始化S态各种中断，包括时钟中断和plic外部中断，在这里的Qemu和K210开发板会有不同；
  - K210对指令`rdtime`报错非法指令，且无法通过tval取得指令值，故K210无法通过riscv::register::time::read()读当前时间；Qemu无此问题；
  - 通过联合opensbi调试，riscv当硬件决定触发时钟中断时，会将sip寄存器的STIP位设置为1；当一条指令执行完毕后，如果发现STIP为1，此时如果sie 的STIE位也为1，会进入S态时钟中断的处理程序；
  - 当在M态不进行时钟中断委派到S态时，Qemu的M态可接收到S态时钟中断；
  - K210的M态无法收到S态中断，时钟中断和软件中断可以委派到S态来收, 而PLIC外部中断即使委派了也不行, 真是大坑! 同时也要感谢前面童鞋踩过坑的提示；

* PLIC外部中断是uart串口输出和virtio-blk-device加载文件系统的关键部分；
  - Qemu UART0_IRQ=10, K210的串口ID为33；
  - 通过MMIO地址对平台级中断控制器PLIC的寄存器进行设置：设置中断源的优先级，分0～7级，7是最高级；设置中断target的全局阀值［0..7]， <= threshold会被屏蔽；
  - 使能target中某个给定ID的中断，中断ID可查找qemu/include/hw/riscv/virt.h；注意一个核的不同权限模式是不同Target，算出的Target不同则操作的地址也不同，跨步0x80；

|Target:| 0 | 1 | 2 | | 3 | 4 | 5 |
|-|-|-|-|-|-|-|-|
|Hart0:| M | S | U | Hart1:| M | S | U |

这里基于opensbi后一般运行于Hart0 S态，故为Target1

* PLIC中断初始化完成后，初始化串口中断，Qemu virt串口基地址是0x1000_0000，而K210是0x38000000；
- 串口中断处理函数，在每个字符从键盘输入时，输出打印出来；

* 接着处理虚拟内存和文件系统

* 由Qemu启动opensbi，装载kernel并引导_start函数，初始化日志log打印，物理内存初始化，进入硬件初始化；
* 后以slice的方式载入ramfs文件系统到内存指定地址，打开该SimpleFileSystem的文件系统并通过linux_loader调用用户程序busybox执行；

* 解析由rcore-fs-fuse生成的Simple FileSystem，通过SimpleFileSystem::open()来打开内存中的文件系统，读取文件和目录；
* 最后通过linux_loader::run busybox sh

* 之前这部分工作由uefi bootloader的rboot把initramfs放到内存的指定地址；
* 内存要初始化好，这里使用Qemu的virtio块设备，故也需要初始化好；

 
移植未完...<br>
系统运行效果演示：<br>


在移植过程中，得到老师和童鞋们的很多帮助！感谢！
