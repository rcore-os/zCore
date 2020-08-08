//! Linux LibOS
//! - run process and manage trap/interrupt/syscall
#![no_std]
#![feature(asm)]
#![feature(global_asm)]
#![deny(warnings, unused_must_use, missing_docs)]

extern crate alloc;
#[macro_use]
extern crate log;

use {
    alloc::{boxed::Box, string::String, sync::Arc, vec::Vec},
    kernel_hal::{GeneralRegs, MMUFlags},
    linux_object::{
        fs::{vfs::FileSystem, INodeExt},
        loader::LinuxElfLoader,
        process::ProcessExt,
        thread::ThreadExt,
    },
    linux_syscall::Syscall,
    zircon_object::task::*,
};

/// Create and run main Linux process
pub fn run(args: Vec<String>, envs: Vec<String>, rootfs: Arc<dyn FileSystem>) -> Arc<Process> {
    let job = Job::root();
    let proc = Process::create_linux(&job, rootfs.clone()).unwrap();
    let thread = Thread::create_linux(&proc).unwrap();
    let loader = LinuxElfLoader {
        #[cfg(feature = "std")]
        syscall_entry: kernel_hal_unix::syscall_entry as usize,
        #[cfg(not(feature = "std"))]
        syscall_entry: 0,
        stack_pages: 8,
        root_inode: rootfs.root_inode(),
    };
    let inode = rootfs.root_inode().lookup(&args[0]).unwrap();
    let data = inode.read_as_vec().unwrap();
    let (entry, sp) = loader.load(&proc.vmar(), &data, args, envs).unwrap();

    thread
        .start(entry, sp, 0, 0, spawn)
        .expect("failed to start main thread");
    proc
}

/// Run and Manage thread
///
/// loop:
/// - wait for the thread to be ready
/// - get user thread context
/// - enter user mode
/// - handle trap/interrupt/syscall according to the return value
/// - return the context to the user thread
fn spawn(thread: Arc<Thread>) {
    let vmtoken = thread.proc().vmar().table_phys();
    let future = async move {
        loop {
            // wait
            let mut cx = thread.wait_for_run().await;
            // run
            trace!("go to user: {:#x?}", cx);
            kernel_hal::context_run(&mut cx);
            trace!("back from user: {:#x?}", cx);
            // handle trap/interrupt/syscall
            let mut exit = false;
            match cx.trap_num {
                0x100 => exit = handle_syscall(&thread, &mut cx.general).await,
                0x20..=0x3f => {
                    kernel_hal::InterruptManager::handle(cx.trap_num as u8);
                    if cx.trap_num == 0x20 {
                        kernel_hal::yield_now().await;
                    }
                }
                0xe => {
                    let vaddr = kernel_hal::fetch_fault_vaddr();
                    let flags = if cx.error_code & 0x2 == 0 {
                        MMUFlags::READ
                    } else {
                        MMUFlags::WRITE
                    };
                    error!("page fualt from user mode {:#x} {:#x?}", vaddr, flags);
                    let vmar = thread.proc().vmar();
                    match vmar.handle_page_fault(vaddr, flags) {
                        Ok(()) => {}
                        Err(_) => {
                            panic!("Page Fault from user mode {:#x?}", cx);
                        }
                    }
                }
                _ => panic!("not supported interrupt from user mode. {:#x?}", cx),
            }
            thread.end_running(cx);
            if exit {
                break;
            }
        }
    };
    kernel_hal::Thread::spawn(Box::pin(future), vmtoken);
}

/// syscall handler entry: create a struct `syscall: Syscall`, and call `syscall.syscall()`
async fn handle_syscall(thread: &Arc<Thread>, regs: &mut GeneralRegs) -> bool {
    trace!("syscall: {:#x?}", regs);
    let num = regs.rax as u32;
    let args = [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9];
    let mut syscall = Syscall {
        thread,
        #[cfg(feature = "std")]
        syscall_entry: kernel_hal_unix::syscall_entry as usize,
        #[cfg(not(feature = "std"))]
        syscall_entry: 0,
        spawn_fn: spawn,
        regs,
        exit: false,
    };
    let ret = syscall.syscall(num, args).await;
    let exit = syscall.exit;
    regs.rax = ret as usize;
    exit
}
