use {
    linux_syscall::Syscall,
    zircon_object::task::*,
    zircon_object::{object::KernelObject, ZxError, ZxResult},
    kernel_hal::context::UserContext,
};

use riscv::register::scause::{Exception, Trap};
use riscv::register::{scause, stval};

pub async fn handler_user_trap(thread: &CurrentThread, cx: &mut UserContext) -> ZxResult {
    let pid = thread.proc().id();

    let trap_cause = scause::read();
    if trap_cause.is_interrupt() {
        kernel_hal::interrupt::handle_irq(trap_cause.code());
        // Timer
        if trap_cause == Interrupt::SupervisorTimer {
            kernel_hal::thread::yield_now().await;
        }
    } else {
        match trap_cause {
            // syscall
            Exception::UserEnvCall => handle_syscall(thread, cx).await,
            // PageFault
            Exception::InstructionPageFault | 
            Exception::LoadPageFault | 
            Exception::StorePageFault => {
                let (vaddr, flags) = kernel_hal::context::fetch_page_fault_info(trap_num);
                warn!(
                    "page fault from user mode @ {:#x}({:?}), pid={}",
                    vaddr, flags, pid
                );
                let vmar = thread.proc().vmar();
                if let Err(err) = vmar.handle_page_fault(vaddr, flags) {
                    error!(
                        "Failed to handle page Fault from user mode @ {:#x}({:?}): {:?}\n{:#x?}",
                        vaddr, flags, err, cx
                    );
                    return Err(err);
                }
            }
            _ => {
                error!(
                    "not supported pid: {} exception {} from user mode. {:#x?}",
                    pid, trap_num, cx
                );
                return Err(ZxError::NOT_SUPPORTED);
            }
        }
    }


    Ok(())
}

async fn handle_syscall(thread: &CurrentThread, cx: &mut UserContext) {
    trace!("syscall: {:#x?}", cx.general);
    let num = cx.general.a7 as u32;
    let args = [
        cx.general.a0,
        cx.general.a1,
        cx.general.a2,
        cx.general.a3,
        cx.general.a4,
        cx.general.a5,
    ];
    // add before fork
    cx.sepc += 4;

    //注意, 此时的regs没有原context所有权，故无法通过此regs修改寄存器
    //let regs = &mut (cx.general as GeneralRegs);

    let mut syscall = Syscall {
        thread,
        #[cfg(feature = "std")]
        syscall_entry: kernel_hal::context::syscall_entry as usize,
        #[cfg(not(feature = "std"))]
        syscall_entry: 0,
        context: cx,
        thread_fn,
    };
    cx.general.a0 = syscall.syscall(num, args).await as usize;
}
