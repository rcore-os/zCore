use std::cell::Cell;

#[export_name = "hal_set_user_fsbase"]
pub fn set_user_fsbase(fsbase: usize) {
    USER_FSBASE.with(|fs| fs.set(fsbase));
    unsafe {
        assert_eq!(fsbase, (fsbase as *const usize).read());
        let kernel_gs = get_gsbase();
        ((fsbase + 48) as *mut usize).write(kernel_gs);
    }
}

/// Switch TLS from user to kernel.
pub unsafe fn switch_to_kernel() {
    let kernel_gsbase: usize;
    asm!("mov %gs:48, $0" : "=r"(kernel_gsbase) ::: "volatile");
    set_gsbase(kernel_gsbase);
}

/// Switch TLS from kernel to user.
pub unsafe fn switch_to_user() {
    let user_gsbase = USER_FSBASE.with(|f| f.get());
    set_gsbase(user_gsbase);
}

thread_local! {
    static USER_FSBASE: Cell<usize> = Cell::new(0);
}

unsafe fn set_gsbase(gsbase: usize) {
    // Ref: https://gist.github.com/aras-p/5389747
    asm!("syscall" :: "{eax}"(0x3000003), "{rdi}"(gsbase) :: "volatile");
}

unsafe fn get_gsbase() -> usize {
    // Ref:
    // - https://github.com/DynamoRIO/dynamorio/issues/1568#issuecomment-239819506
    // - https://github.com/apple/darwin-libpthread/blob/03c4628c8940cca6fd6a82957f683af804f62e7f/src/internal.h#L241
    let pthread_self: usize;
    asm!("mov %gs:0, $0" : "=r"(pthread_self) ::: "volatile");
    pthread_self + 224
}

/// Register signal handler for SIGSEGV (Segmentation Fault).
///
///
unsafe fn register_sigsegv_handler() {
    let sa = libc::sigaction {
        sa_sigaction: handler as usize,
        sa_flags: libc::SA_SIGINFO,
        sa_mask: 0,
    };
    libc::sigaction(libc::SIGSEGV, &sa, core::ptr::null_mut());

    #[repr(C)]
    struct ucontext {
        uc_onstack: i32,
        uc_sigmask: u32,
        uc_stack: [u32; 5],
        uc_link: usize,
        uc_mcsize: usize,
        uc_mcontext: *const mcontext,
    }

    #[repr(C)]
    #[derive(Debug)]
    struct mcontext {
        trapno: u16,
        cpu: u16,
        err: u32,
        faultvaddr: u64,
        rax: u64,
        rbx: u64,
        rcx: u64,
        rdx: u64,
        rdi: u64,
        rsi: u64,
        rbp: u64,
        rsp: u64,
        r8: u64,
        r9: u64,
        r10: u64,
        r11: u64,
        r12: u64,
        r13: u64,
        r14: u64,
        r15: u64,
        rip: u64,
        rflags: u64,
        cs: u64,
        fs: u64,
        gs: u64,
    }

    /// Signal handler for when code tries to use %fs.
    ///
    /// Ref: https://github.com/NuxiNL/cloudabi-utils/blob/38d845bc5cc6fcf441fe0d3c2433f9298cbeb760/src/libemulator/tls.c#L30-L53
    unsafe extern "C" fn handler(
        _sig: libc::c_int,
        _si: *const libc::siginfo_t,
        uc: *const ucontext,
    ) {
        let rip = (*(*uc).uc_mcontext).rip as *mut u8;
        let rip_value = rip.read();
        trace!("catch SIGSEGV: rip={:?}, opcode={:#x}", rip, rip_value);
        match rip_value {
            // Instruction starts with 0x64, meaning it tries to access %fs. By
            // changing the first byte to 0x65, it uses %gs instead.
            0x64 => rip.write(0x65),
            // Instruction has already been patched up, but it may well be the
            // case that this was done by another CPU core. There is nothing
            // else we can do than return and try again. This may cause us to
            // get stuck indefinitely.
            0x65 => {}
            // Segmentation violation on an instruction that does not try to
            // access %fs. Reset the handler to its default action, so that the
            // segmentation violation is rethrown.
            _ => {
                let sa = libc::sigaction {
                    sa_sigaction: libc::SIG_DFL,
                    sa_flags: 0,
                    sa_mask: 0,
                };
                libc::sigaction(libc::SIGSEGV, &sa, core::ptr::null_mut());
            }
        }
    }
}
