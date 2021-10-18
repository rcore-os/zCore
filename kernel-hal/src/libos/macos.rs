//! Register signal handler for SIGSEGV (Segmentation Fault).

use nix::libc;
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, SIGSEGV};

#[repr(C)]
struct Ucontext {
    uc_onstack: i32,
    uc_sigmask: u32,
    uc_stack: [u32; 5],
    uc_link: usize,
    uc_mcsize: usize,
    uc_mcontext: *const Mcontext,
}

#[repr(C)]
#[derive(Debug)]
struct Mcontext {
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
extern "C" fn sig_handler(_sig: libc::c_int, _si: *mut libc::siginfo_t, uc: *mut libc::c_void) {
    unsafe {
        let uc = uc as *mut Ucontext;
        let mut rip = (*(*uc).uc_mcontext).rip as *mut u8;
        // skip data16 prefix
        while rip.read() == 0x66 {
            rip = rip.add(1);
        }
        match rip.read() {
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
                // switch back to kernel gs
                asm!(
                    "
                    mov rdi, gs:48
                    syscall
                    ",
                    in("eax") 0x3000003,
                    out("rdi") _,
                    out("rcx") _,
                    out("r11") _,
                );
                panic!("catch SIGSEGV: {:#x?}", *(*uc).uc_mcontext);
            }
        }
    }
}

pub unsafe fn register_sigsegv_handler() {
    let sa = SigAction::new(
        SigHandler::SigAction(sig_handler),
        SaFlags::SA_SIGINFO,
        SigSet::empty(),
    );
    sigaction(SIGSEGV, &sa).expect("failed to register signal handler!");
}
