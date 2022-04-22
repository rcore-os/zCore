# 提供的全局符号
# -------------------------------
# /// 主核入口
# fn _start(hartid: usize, device_tree_paddr: usize) -> !;
    .global _start
# /// 副核入口
# fn _secondary_hart_start(hartid: usize) -> !;
    .global _secondary_hart_start
# /// 地址空间跃迁
# fn jump_heigher(offset: usize);
    .global _jump_higher

# 依赖的全局符号
# -------------------------------
# /// 主核入口
# fn primary_rust_main(hartid: usize, device_tree_paddr: usize) -> !;
#
# /// 副核入口
# fn secondary_rust_main(hartid: usize) -> !;

    .section .text.entry

# 主核入口
# 清零 bss 段，构造启动页表，然后跳转到高地址映射的主核主函数
# -------------------------------
_start:                      # fn _start(hartid: usize, device_tree_paddr: usize) -> ! {{
    csrw sie,  zero          #     $sie = 0; // 关中断
    call select_stack        #     select_stack(hartid);
    j    primary_rust_main   #     primary_rust_main(hartid, device_tree_paddr)
                             # }}
# -------------------------------

# 副核入口
# 构造启动页表，然后跳转到高地址映射的副核主函数
# -------------------------------
_secondary_hart_start:       # fn _secondary_hart_start(hartid: usize) -> ! {{
    csrw sie, zero           #     $sie = 0; // 关中断
    call select_stack        #     select_stack(hartid);
    j    secondary_rust_main #     secondary_rust_main(hartid)
                             # }}
# -------------------------------

# 根据线程号设置启动栈
# -------------------------------
select_stack:                # fn select_stack(hartid: usize) {{
    mv   t0, a0              #     $t0 = hartid;
    la   sp, bootstacktop    #     $sp = bootstacktop;
    beqz t0, 2f              #     if $t0 != 0 {{
    li   t1, -4096*16        #         $t1 = -4096*16;
                             #         loop {{
1:  add  sp, sp, t1          #             $sp += $t1;
    addi t0, t0, -1          #             $t0 -= 1;
    bgtz t0, 1b              #             if $t0 > 0 {{ continue; }}
                             #         }}
                             #     }}
2:  ret                      # }}
# -------------------------------

# 从地址空间的低处跳到地址空间高处对应位置并挪动栈指针
# -------------------------------
_jump_higher:                # fn jump_heigher(offset: usize) {{
    add  ra, ra, a0          #     $t0 += offset;
    add  sp, sp, a0          #     $sp += offset;
    ret                      # }}
# -------------------------------

    .section .bss.bootstack
    .align 12
bootstack:
    .space 4096 * 160
bootstacktop:
