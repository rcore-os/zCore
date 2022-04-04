//! Run Linux process and manage trap/interrupt/syscall.

use alloc::{boxed::Box, string::String, sync::Arc, vec::Vec};
use linux_object::signal::{SigInfo, SiginfoFields, SignalUserContext, Sigset};
use core::{future::Future, pin::Pin};

use kernel_hal::context::{TrapReason, UserContext, UserContextField};
use linux_object::fs::{vfs::FileSystem, INodeExt};
use linux_object::thread::{CurrentThreadExt, ThreadExt};
use linux_object::{loader::LinuxElfLoader, process::ProcessExt};
use zircon_object::task::{CurrentThread, Job, Process, Thread, ThreadState};
use zircon_object::{object::KernelObject, ZxError, ZxResult};

/// Create and run main Linux process
pub fn run(args: Vec<String>, envs: Vec<String>, rootfs: Arc<dyn FileSystem>) -> Arc<Process> {
    info!("Run Linux process: args={:?}, envs={:?}", args, envs);

    let job = Job::root();
    let proc = Process::create_linux(&job, rootfs.clone()).unwrap();
    let thread = Thread::create_linux(&proc).unwrap();
    let loader = LinuxElfLoader {
        syscall_entry: kernel_hal::context::syscall_entry as usize,
        stack_pages: 8,
        root_inode: rootfs.root_inode(),
    };

    let inode = rootfs.root_inode().lookup(&args[0]).unwrap();
    let data = inode.read_as_vec().unwrap();
    let path = args[0].clone();

    let pg_token = kernel_hal::vm::current_vmtoken();
    debug!("current pgt = {:#x}", pg_token);
    //调用zircon-object/src/task/thread.start设置好要执行的thread
    let (entry, sp) = loader.load(&proc.vmar(), &data, args, envs, path).unwrap();

    thread
        .start_with_entry(entry, sp, 0, 0, thread_fn)
        .expect("failed to start main thread");
    proc
}

fn thread_fn(thread: CurrentThread) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
    Box::pin(run_user(thread))
}

/// The function of a new thread.
///
/// loop:
/// - wait for the thread to be ready
/// - get user thread context
/// - enter user mode
/// - handle trap/interrupt/syscall according to the return value
/// - return the context to the user thread
async fn run_user(thread: CurrentThread) {
    kernel_hal::thread::set_tid(thread.id(), thread.proc().id());
    loop {
        // wait
        let mut ctx = thread.wait_for_run().await;
        if thread.state() == ThreadState::Dying {
            break;
        }

        // check the signal and handle
        let signals = thread.inner().lock_linux().signals;
        let sigmask = thread.inner().lock_linux().signal_mask;
        let handling_signal = thread.inner().lock_linux().handling_signal;
        if signals.mask_with(&sigmask).is_not_empty() && handling_signal.is_none() {
            handle_signal(&thread, &mut *ctx, sigmask);
        }

        // run
        trace!("go to user: {:#x?}", ctx);
        ctx.enter_uspace();
        trace!("back from user: {:#x?}", ctx);

        // handle trap/interrupt/syscall
        if let Err(err) = handle_user_trap(&thread, ctx).await {
            thread.exit_linux(err as i32);
        }
    }
}

fn handle_signal(thread: &CurrentThread, ctx: *mut UserContext, sigmask: Sigset) {
    let action = thread.proc().linux().signal_action(linux_object::signal::Signal::SIGRT33);
    let signal_info = SigInfo {
        signo: 0,
        errno: 0,
        code: linux_object::signal::SignalCode::TKILL,
        field: SiginfoFields::default()
    };
    let mut signal_context = SignalUserContext::default();
    signal_context.sig_mask = sigmask.val() as u128;
    thread.lock_linux().handling_signal = Some(linux_object::signal::Signal::SIGRT33 as u32);
    // backup current context and set new context
    unsafe {
        thread.backup_context((*ctx).clone());
        let sp = (*ctx).get_field(UserContextField::StackPointer) - 0x200;
        let sp = push_stack::<SigInfo>(sp, signal_info);
        let siginfo_ptr = sp;
        let pc = (*ctx).get_field(UserContextField::InstrPointer);
        signal_context.context.set_pc(pc);
        let sp = push_stack::<SignalUserContext>(sp, signal_context);
        (*ctx).setup_uspace(action.handler, sp, &[
            linux_object::signal::Signal::SIGRT33 as usize, siginfo_ptr, sp
        ]);
        (*ctx).set_ra(action.restorer);
        (*ctx).enter_uspace();
    }
}

/// Push stack
pub unsafe fn push_stack<T>(stack_top: usize, val: T) -> usize {
    let stack_top = (stack_top as *mut T).sub(1);
    *stack_top = val;
    stack_top as usize
}

async fn handle_user_trap(thread: &CurrentThread, mut ctx: Box<UserContext>) -> ZxResult {
    let reason = ctx.trap_reason();

    if let TrapReason::Syscall = reason {
        let num = syscall_num(&ctx);
        let args = syscall_args(&ctx);
        ctx.advance_pc(reason);
        thread.put_context(ctx);
        let mut syscall = linux_syscall::Syscall {
            thread,
            thread_fn,
            syscall_entry: kernel_hal::context::syscall_entry as usize,
        };
        let ret = syscall.syscall(num as u32, args).await as usize;
        thread.with_context(|ctx| ctx.set_field(UserContextField::ReturnValue, ret))?;
        return Ok(());
    }

    thread.put_context(ctx);

    let pid = thread.proc().id();
    match reason {
        TrapReason::Interrupt(vector) => {
            kernel_hal::interrupt::handle_irq(vector);
            kernel_hal::thread::yield_now().await;
            Ok(())
        }
        TrapReason::PageFault(vaddr, flags) => {
            warn!(
                "page fault from user mode @ {:#x}({:?}), pid={}",
                vaddr, flags, pid
            );
            let vmar = thread.proc().vmar();
            vmar.handle_page_fault(vaddr, flags).map_err(|err| {
                error!(
                    "failed to handle page fault from user mode @ {:#x}({:?}): {:?}\n{:#x?}",
                    vaddr,
                    flags,
                    err,
                    thread.context_cloned(),
                );
                err
            })
        }
        _ => {
            error!(
                "unsupported trap from user mode: {:x?}, pid={}, {:#x?}",
                reason,
                pid,
                thread.context_cloned(),
            );
            Err(ZxError::NOT_SUPPORTED)
        }
    }
}

fn syscall_num(ctx: &UserContext) -> usize {
    let regs = ctx.general();
    cfg_if! {
        if #[cfg(target_arch = "x86_64")] {
            regs.rax
        } else if #[cfg(target_arch = "aarch64")] {
            regs.x8
        } else if #[cfg(target_arch = "riscv64")] {
            regs.a7
        } else {
            unimplemented!()
        }
    }
}

fn syscall_args(ctx: &UserContext) -> [usize; 6] {
    let regs = ctx.general();
    cfg_if! {
        if #[cfg(target_arch = "x86_64")] {
            [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9]
        } else if #[cfg(target_arch = "aarch64")] {
            [regs.x0, regs.x1, regs.x2, regs.x3, regs.x4, regs.x5]
        } else if #[cfg(target_arch = "riscv64")] {
            [regs.a0, regs.a1, regs.a2, regs.a3, regs.a4, regs.a5]
        } else {
            unimplemented!()
        }
    }
}
