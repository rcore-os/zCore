#[export_name = "hal_set_user_fsbase"]
pub fn set_user_fsbase(fsbase: usize) {
    unsafe {
        set_gsbase(fsbase);
    }
}

pub unsafe fn switch_to_kernel() {
    swap_fs_gs();
}

pub unsafe fn switch_to_user() {
    swap_fs_gs();
}

/// Swap FSBASE and GSBASE
unsafe fn swap_fs_gs() {
    let fs = get_fsbase();
    let gs = get_gsbase();
    set_fsbase(gs);
    set_gsbase(fs);
}

unsafe fn set_fsbase(fsbase: usize) {
    const ARCH_SET_FS: i32 = 0x1002;
    sys_arch_prctl(ARCH_SET_FS, fsbase);
}

unsafe fn set_gsbase(gsbase: usize) {
    const ARCH_SET_GS: i32 = 0x1001;
    sys_arch_prctl(ARCH_SET_GS, gsbase);
}

unsafe fn get_fsbase() -> usize {
    let mut fsbase: usize = 0;
    const ARCH_GET_FS: i32 = 0x1003;
    sys_arch_prctl(ARCH_GET_FS, &mut fsbase as *mut _ as usize);
    fsbase
}

unsafe fn get_gsbase() -> usize {
    let mut gsbase: usize = 0;
    const ARCH_GET_GS: i32 = 0x1004;
    sys_arch_prctl(ARCH_GET_GS, &mut gsbase as *mut _ as usize);
    gsbase
}

unsafe fn sys_arch_prctl(code: i32, addr: usize) {
    asm!("syscall"
        :
        : "{rax}"(libc::SYS_arch_prctl), "{rdi}"(code), "{rsi}"(addr)
        : "rcx" "r11" "memory"
        : "volatile");
}
