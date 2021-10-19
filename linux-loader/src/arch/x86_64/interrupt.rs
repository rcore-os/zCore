use {
    linux_syscall::Syscall,
    zircon_object::task::*,
    zircon_object::{object::KernelObject, ZxError, ZxResult},
    kernel_hal::context::UserContext,
};
use super::*;
use kernel_hal::context::GeneralRegs;
use crate::thread_fn;

pub async fn handler_user_trap(thread: &CurrentThread, cx: &mut UserContext) -> ZxResult {
    let pid = thread.proc().id();

    match cx.trap_num {
        SYSCALL => handle_syscall(thread, &mut cx.general).await,
        X86_INT_BASE..=X86_INT_MAX => {
            kernel_hal::interrupt::handle_irq(cx.trap_num);
            // TODO: configurable
            if cx.trap_num == X86_INT_APIC_TIMER {
                kernel_hal::thread::yield_now().await;
            }
        }
        PAGE_FAULT => {
            let (vaddr, flags) = kernel_hal::context::fetch_page_fault_info(cx.error_code);
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
            error!("not supported interrupt from user mode. {:#x?}", cx);
            return Err(ZxError::NOT_SUPPORTED);
        }
    }
    Ok(())
}

async fn handle_syscall(thread: &CurrentThread, regs: &mut GeneralRegs) {
    trace!("syscall: {:#x?}", regs);
    let num = regs.rax as u32;
    let args = [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9];
    let mut syscall = Syscall {
        thread,
        #[cfg(feature = "std")]
        syscall_entry: kernel_hal::context::syscall_entry as usize,
        #[cfg(not(feature = "std"))]
        syscall_entry: 0,
        thread_fn,
        regs,
    };
    regs.rax = syscall.syscall(num, args).await as usize;
}