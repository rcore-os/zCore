use core::arch::global_asm;

global_asm!(include_str!("trap.S"));
global_asm!(include_str!(concat!(env!("OUT_DIR"), "/vector.S")));

/// Trap frame of kernel interrupt
///
/// # Trap handler
///
/// You need to define a handler function like this:
///
/// ```
/// use trapframe::TrapFrame;
///
/// #[no_mangle]
/// extern "sysv64" fn trap_handler(tf: &mut TrapFrame) {
///     match tf.trap_num {
///         3 => {
///             println!("TRAP: BreakPoint");
///             tf.rip += 1;
///         }
///         _ => panic!("TRAP: {:#x?}", tf),
///     }
/// }
/// ```
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct TrapFrame {
    // Pushed by 'trap.S'
    pub rax: usize,
    pub rbx: usize,
    pub rcx: usize,
    pub rdx: usize,
    pub rsi: usize,
    pub rdi: usize,
    pub rbp: usize,
    pub rsp: usize,
    pub r8: usize,
    pub r9: usize,
    pub r10: usize,
    pub r11: usize,
    pub r12: usize,
    pub r13: usize,
    pub r14: usize,
    pub r15: usize,
    pub _pad: usize,

    // Pushed by 'vector.S'
    pub trap_num: usize,
    pub error_code: usize,

    // Pushed by CPU
    pub rip: usize,
    pub cs: usize,
    pub rflags: usize,
}
