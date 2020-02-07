/// Switch TLS from user to kernel.
///
/// # Safety
/// This function should be called once when come from user.
pub unsafe fn switch_to_kernel() {
    const ARCH_SET_FS: i32 = 0x1002;
    asm!("mov rsi, fs:48; syscall"
        :
        : "{eax}"(libc::SYS_arch_prctl), "{edi}"(ARCH_SET_FS)
        : "rcx" "r11" "memory"
        : "volatile" "intel");
}
