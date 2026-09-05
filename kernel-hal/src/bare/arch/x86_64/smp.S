# Real-mode trampoline derived from rcore-os/x86-smpboot, boot_ap.S.
# Loaded at physical 0x6000; shared arguments are changed only after an AP
# has switched to its private stack and acknowledged entry into Rust.
.equ ap64, zcore_ap64 - zcore_ap_start + 0x6000
.equ gdt_addr, zcore_ap_gdt - zcore_ap_start + 0x6000
.equ gdtr_addr, zcore_ap_gdtr - zcore_ap_start + 0x6000
.global zcore_ap_start
.global zcore_ap_end
.section .text
.code16
zcore_ap_start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov eax, cr4
    or eax, (1 << 5) | (1 << 7) | (1 << 9) | (1 << 10)
    mov cr4, eax
    mov eax, [0x6ff8]
    mov cr3, eax
    mov ecx, 0xC0000080
    rdmsr
    or eax, (1 << 8) | (1 << 11)
    wrmsr
    mov eax, cr0
    and eax, ~((1 << 2) | (1 << 3))
    or eax, (1 << 0) | (1 << 1) | (1 << 16) | (1 << 31)
    mov cr0, eax
    mov esp, 0x6fe0
    lgdt [gdtr_addr]
    push 0x8
    lea eax, [ap64]
    push eax
    retf
.code64
zcore_ap64:
    xor ax, ax
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov rsp, [0x6fe8]
    mov rax, [0x6ff0]
    call rax
1:  cli
    hlt
    jmp 1b
.balign 8
zcore_ap_gdt:
    .quad 0
    .quad 0x00209A0000000000
    .quad 0x0000920000000000
zcore_ap_gdtr:
    .word zcore_ap_gdtr - zcore_ap_gdt - 1
    .long gdt_addr
zcore_ap_end:
