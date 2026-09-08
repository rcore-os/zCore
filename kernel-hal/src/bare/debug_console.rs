//! QEMU-only kernel diagnostics, separate from the guest console UART.

pub fn write(text: &str) {
    #[cfg(target_arch = "x86_64")]
    for byte in text.bytes() {
        // QEMU's isa-debugcon; this port is independent of the guest UART.
        unsafe {
            core::arch::asm!("out dx, al", in("dx") 0xe9u16, in("al") byte, options(nomem, nostack, preserves_flags))
        };
    }
    #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
    {
        // SYS_WRITE0 is routed to the QEMU semihosting chardev. Chunking avoids
        // allocation and bounds stack use even while reporting a kernel panic.
        for chunk in text.as_bytes().chunks(255) {
            let mut buffer = [0u8; 256];
            buffer[..chunk.len()].copy_from_slice(chunk);
            unsafe {
                #[cfg(target_arch = "aarch64")]
                core::arch::asm!("hlt #0xf000", inout("x0") 4usize => _, in("x1") buffer.as_ptr(), options(nostack));
                #[cfg(target_arch = "riscv64")]
                core::arch::asm!(
                    ".option push", ".option norvc", ".balign 16",
                    "slli zero, zero, 31", "ebreak", "srai zero, zero, 7",
                    ".option pop",
                    inout("a0") 4usize => _, in("a1") buffer.as_ptr(), options(nostack)
                );
            }
        }
    }
}
