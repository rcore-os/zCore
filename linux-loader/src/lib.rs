//! Linux LibOS
//! - run process and manage trap/interrupt/syscall
#![no_std]
#![feature(asm)]
#![deny(warnings, unused_must_use, missing_docs)]

extern crate alloc;
#[macro_use]
extern crate log;

use {
    alloc::{boxed::Box, string::String, sync::Arc, vec::Vec},
    core::{future::Future, pin::Pin},
    linux_object::{
        fs::{vfs::FileSystem, INodeExt},
        loader::LinuxElfLoader,
        process::ProcessExt,
        thread::{CurrentThreadExt, ThreadExt},
    },
    linux_syscall::Syscall,
    zircon_object::task::*,
    zircon_object::{object::KernelObject, ZxError, ZxResult},
};

use kernel_hal::context::UserContext;

#[cfg(target_arch = "x86_64")]
use kernel_hal::context::GeneralRegs;

/// Create and run main Linux process
pub fn run(args: Vec<String>, envs: Vec<String>, rootfs: Arc<dyn FileSystem>) -> Arc<Process> {
    info!("Run Linux process: args={:?}, envs={:?}", args, envs);

    let job = Job::root();
    let proc = Process::create_linux(&job, rootfs.clone()).unwrap();
    let thread = Thread::create_linux(&proc).unwrap();
    let loader = LinuxElfLoader {
        #[cfg(feature = "libos")]
        syscall_entry: kernel_hal::context::syscall_entry as usize,
        #[cfg(not(feature = "libos"))]
        syscall_entry: 0,
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
        .start(entry, sp, 0, 0, thread_fn)
        .expect("failed to start main thread");
    proc
}

/// The function of a new thread.
///
/// loop:
/// - wait for the thread to be ready
/// - get user thread context
/// - enter user mode
/// - handle trap/interrupt/syscall according to the return value
/// - return the context to the user thread
async fn new_thread(thread: CurrentThread) {
    kernel_hal::thread::set_tid(thread.id(), thread.proc().id());
    loop {
        // wait
        let mut cx = thread.wait_for_run().await;
        if thread.state() == ThreadState::Dying {
            break;
        }

        // run
        trace!("go to user: {:#x?}", cx);
        kernel_hal::context::context_run(&mut cx);
        trace!("back from user: {:#x?}", cx);
        // handle trap/interrupt/syscall

        if let Err(err) = handler_user_trap(&thread, &mut cx).await {
            thread.exit_linux(err as i32);
        }

        thread.end_running(cx);
    }
}

fn thread_fn(thread: CurrentThread) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
    Box::pin(new_thread(thread))
}

async fn handler_user_trap(thread: &CurrentThread, cx: &mut UserContext) -> ZxResult {
    let pid = thread.proc().id();

    #[cfg(target_arch = "x86_64")]
    match cx.trap_num {
        0x100 => handle_syscall(thread, &mut cx.general).await,
        0x20..=0xff => {
            kernel_hal::interrupt::handle_irq(cx.trap_num);
            // TODO: configurable
            if cx.trap_num == 0xf1 {
                kernel_hal::thread::yield_now().await;
            }
        }
        0xe => {
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

    // UserContext
    #[cfg(target_arch = "riscv64")]
    {
        let trap_num = kernel_hal::context::fetch_trap_num(cx);
        let is_interrupt = ((trap_num >> (core::mem::size_of::<usize>() * 8 - 1)) & 1) == 1;
        let trap_num = trap_num & 0xfff;
        if is_interrupt {
            kernel_hal::interrupt::handle_irq(trap_num);
            // Timer
            if trap_num == 5 {
                kernel_hal::thread::yield_now().await;
            }
        } else {
            match trap_num {
                // syscall
                8 => handle_syscall(thread, cx).await,
                // PageFault
                12 | 13 | 15 => {
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
    }

    Ok(())
}

/// syscall handler entry
#[cfg(target_arch = "x86_64")]
async fn handle_syscall(thread: &CurrentThread, regs: &mut GeneralRegs) {
    trace!("syscall: {:#x?}", regs);
    let num = regs.rax as u32;
    let args = [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9];
    let mut syscall = Syscall {
        thread,
        #[cfg(feature = "libos")]
        syscall_entry: kernel_hal::context::syscall_entry as usize,
        #[cfg(not(feature = "libos"))]
        syscall_entry: 0,
        thread_fn,
        regs,
    };
    regs.rax = syscall.syscall(num, args).await as usize;
}

#[cfg(target_arch = "riscv64")]
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
        #[cfg(feature = "libos")]
        syscall_entry: kernel_hal::context::syscall_entry as usize,
        #[cfg(not(feature = "libos"))]
        syscall_entry: 0,
        context: cx,
        thread_fn,
    };
    cx.general.a0 = syscall.syscall(num, args).await as usize;
}
