# 提供的全局符号
# --------------------------------------
# 主核入口
# fn _start(hartid: usize, device_tree_paddr: usize) -> !;
    .globl _start
# 副核入口
# fn _secondary_hart_start(hartid: usize) -> !;
    .globl _secondary_hart_start
# 启动栈（160 页）
# RISC-V 架构栈从高地址向低地址增长
# const bootstack:  *const u8; # 栈底
# const bootstacktop: *mut u8; # 栈顶
    .global bootstack
    .global bootstacktop

# 依赖的全局符号
# --------------------------------------
# /// 用于启动的 Sv39 1GiB 页表页
# const boot_page_table_sv39: *const u8;
#
# /// 主核启动函数
# fn primary_rust_main(hartid: usize, device_tree_paddr: usize) -> !;
#
# /// 副核启动函数
# fn secondary_rust_main() -> !;

    .section .text.entry

# 跨页跳转
# 加载 `symbol` 的地址并转换到高地址映射，然后跳转
# --------------------------------------
.macro jump_cross symbol            #
    lui  t0, %hi(\symbol)           #
    addi t0, t0, %lo(\symbol)       #
    jr   t0                         #
.endm                               #
# --------------------------------------

# 主核入口
# 清零 bss 段，构造启动页表，然后跳转到高地址映射的主核主函数
# --------------------------------------
_start:                             # fn _start(hartid: usize, device_tree_paddr: usize) -> ! {{
    csrw sie,  zero                 #     $sie = 0; // 关中断
                                    #     // 清空 bss 段
    la   t0,   sbss                 #     $t0 = sbss;
    la   t1,   ebss                 #     $t1 = ebss;
                                    #     loop {{
1:  sd   zero, (t0)                 #         *$t0 = 0usize;
    addi t0,   t0, 8                #         $t0 += 8;
    bltu t0,   t1, 1b               #         if $t0 < $t1 {{ continue; }}
                                    #     }}
    call       init_vm              #     init_vm(hartid); // 启动虚存，并做一些其他初始化工作
    jump_cross primary_rust_main    #     primary_rust_main(hartid, device_tree_paddr)
                                    # }}
# --------------------------------------

# 副核入口
# 构造启动页表，然后跳转到高地址映射的副核主函数
# --------------------------------------
_secondary_hart_start:              # fn _secondary_hart_start(hartid: usize) -> ! {{
    csrw sie, zero                  #     $sie = 0;        // 关中断
    call       init_vm              #     init_vm(hartid); // 启动虚存，并做一些其他初始化工作
    jump_cross secondary_rust_main  #     secondary_rust_main()
                                    # }}
# --------------------------------------

# 构造启动页表并启用地址映射
# --------------------------------------
init_vm:                            # fn init_vm(hartid: usize) {{
    .equ SATP_MODE_SV39, 8 << 60    #     const SATP_MODE_SV39: usize = 8 << 60;
                                    #     // 构造并使能启动页表
    li   t1,   SATP_MODE_SV39       #     $t1   = SATP_MODE_SV39;
    la   t0,   boot_page_table_sv39 #     $t0   = boot_page_table_sv39;
    srli t0,   t0, 12               #     $t0 >>= 12;
    or   t0,   t0, t1               #     $t0  |= $t1;
    csrw satp, t0                   #     $satp = $t0;
    sfence.vma                      #     // 刷新 TLB
                                    #     // 立即跳到高地址
    li   t1,   PHY_MEM_OFS          #     $t1   = PHY_MEM_OFS;
    la   t0,   1f                   #     $t0   = @1f;
    add  t0,   t0, t1               #     $t0  += $t1;
    jr   t0                         #     // 跳到高映射的虚地址
                                    #     // 设置启动栈
1:  lui  sp,   %hi(bootstacktop)    #     $sp   = bootstacktop;
    mv   t0,   a0                   #     $t0   = hartid;
    beqz t0,   2f                   #     if $t0 != 0 {{
    li   t1,   -4096 * 16           #         $t1  = -4096 * 16;
                                    #         loop {{
1:  add  sp,   sp, t1               #             $sp += $t1;
    addi t0,   t0, -1               #             $t0 -= 1;
    bgtz t0,   1b                   #             if $t0 > 0 {{ continue; }}
                                    #         }}
                                    #     }}
2:  mv   tp,   a0                   #     // 设置线程指针
    csrrsi x0, sstatus, 18          #     // 使能内核访问用户页
    ret                             # }}
# --------------------------------------

    .section .bss.bootstack
    .align 12
bootstack:
    .space 4096 * 160
bootstacktop:
