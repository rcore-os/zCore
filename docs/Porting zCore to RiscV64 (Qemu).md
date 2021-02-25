# Porting zCore to RiscV64 (Qemu)

### zCore系统结构
![](./structure.svg)

riscv64初始移植路径按照：<br>
|Qemu riscv64|-|kernel-hal-bare|-|kernel-hal|-|zircon-object/ linux-object|-|linux-syscall|-|linux-loader|-|busybox|
|-|-|-|-|-|-|-|-|-|-|-|-|-|


* 分析Makefile的结构，弄清楚包括编译和运行的命令执行流；
* 结合Qemu virt和opensbi以及编译出所需文件格式的kernel，执行过程写入Makefile；

* 分析Cargo.toml的结构，理解各个features和依赖库crates的关系，如有些是可选的；
* 创建关于riscv64的Rust target-spec-json编译目标规格描述.json
* 创建riscv64对应的链接脚本.ld以及启动入口汇编.asm

* 重要的修改部分在kernel-hal-bare，arch下添加riscv架构，包括硬件初始化和对opensbi接口调用的封装；

* 先定一个小目标，让OS跑起来打印初始一段字符；
* 这时会有大量的编译错误提示，需要解决，Rust错误提示很详细，会建议如何来修正：
  - 报错可能来自多个方面，包括：riscv64在Cargo.toml中的依赖库，非必需的crates先关掉，添加架构相关所需要的；
  - kernel-hal-bare中缺失的待实现的接口，可见kernel-hal中的定义；参考了arch/x86_64的函数；
  - 与target_arch由x86_64移植到riscv64的cfg，其相关的函数或变量需在代码中补上；
  - 变量及函数的作用范围等需要注意

* Rust的条件编译cfg，`#[cfg(target_arch = "x86_64")]`也需要为riscv64实现一份；

由Qemu启动opensbi，装载kernel并引导_start函数，初始化日志log打印，物理内存初始化，进入硬件初始化；




在移植过程中，得到老师和童鞋们的很多帮助！感谢！
