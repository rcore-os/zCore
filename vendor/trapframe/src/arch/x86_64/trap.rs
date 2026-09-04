use core::arch::global_asm;

global_asm!(include_str!("trap.S"));
global_asm!(include_str!(concat!(env!("OUT_DIR"), "/vector.S")));

/// Register frame saved when an interrupt or exception enters the kernel.
///
/// # Trap handler
///
/// You need to define a handler function like this:
///
/// ```
/// use trapframe::TrapFrame;
///
/// #[unsafe(no_mangle)]
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
    /// Accumulator register.
    pub rax: usize,
    /// Base register.
    pub rbx: usize,
    /// Counter register.
    pub rcx: usize,
    /// Data register.
    pub rdx: usize,
    /// Source index register.
    pub rsi: usize,
    /// Destination index register.
    pub rdi: usize,
    /// Frame pointer register.
    pub rbp: usize,
    /// Stack pointer captured by the assembly entry code.
    pub rsp: usize,
    /// General-purpose register R8.
    pub r8: usize,
    /// General-purpose register R9.
    pub r9: usize,
    /// General-purpose register R10.
    pub r10: usize,
    /// General-purpose register R11.
    pub r11: usize,
    /// General-purpose register R12.
    pub r12: usize,
    /// General-purpose register R13.
    pub r13: usize,
    /// General-purpose register R14.
    pub r14: usize,
    /// General-purpose register R15.
    pub r15: usize,
    /// Alignment slot reserved by the assembly frame layout.
    pub _pad: usize,

    /// Interrupt vector number.
    pub trap_num: usize,
    /// Processor-supplied or synthetic error code.
    pub error_code: usize,

    /// Instruction pointer at the interrupted location.
    pub rip: usize,
    /// Code segment selector at the interrupted location.
    pub cs: usize,
    /// Processor flags at the interrupted location.
    pub rflags: usize,
}
