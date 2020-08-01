#![no_std]
#![feature(asm)]
#![feature(global_asm)]
#![deny(warnings, unused_must_use)]
#![allow(unused_assignments)]

extern crate alloc;
#[macro_use]
extern crate log;

use {
    alloc::{boxed::Box, string::String, sync::Arc, vec::Vec},
    kernel_hal::{InterruptManager, MMUFlags, UserContext},
    linux_object::{
        fs::{vfs::FileSystem, INodeExt},
        loader::LinuxElfLoader,
        process::ProcessExt,
        thread::ThreadExt,
    },
    linux_syscall::Syscall,
    zircon_object::task::*,
};

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

#[cfg(target_arch = "x86_64")]
fn spawn(thread: Arc<Thread>) {
    let vmtoken = thread.proc().vmar().table_phys();
    let future = async move {
        loop {
            let mut cx = thread.wait_for_run().await;
            trace!("go to user: {:#x?}", cx);
            kernel_hal::context_run(&mut cx);
            trace!("back from user: {:#x?}", cx);
            let mut exit = false;
            match cx.trap_num {
                0x100 => exit = handle_syscall(&thread, &mut cx).await,
                0x20..=0x3f => {
                    InterruptManager::handle(cx.trap_num as u8);
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

#[cfg(target_arch = "mips")]
fn spawn(thread: Arc<Thread>) {
    let vmtoken = thread.proc().vmar().table_phys();
    let future = async move {
        loop {
            let mut cx = thread.wait_for_run().await;
            trace!("go to user: {:#x?}", cx);
            kernel_hal::context_run(&mut cx);
            trace!("back from user: {:#x?}", cx);
            let mut exit = false;
            let trap_num = cx.cause;
            match trap_num {
                // _ if InterruptManager::is_page_fault(trap_num) => {
                //     let addr = cp0::bad_vaddr::read_u32() as usize;
                //     if !handle_user_page_fault(&thread, addr) {
                //         // TODO: SIGSEGV
                //         panic!("page fault handle failed");
                //     }
                // }
                // _ if InterruptManager::is_syscall(trap_num) => {
                //     exit = handle_syscall(&thread, &mut cx).await
                // }
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

async fn handle_syscall(thread: &Arc<Thread>, context: &mut UserContext) -> bool {
    let regs = &context.general;
    trace!("syscall: {:#x?}", regs);
    let num = context.get_syscall_num();
    let args = context.get_syscall_args();

    // add before fork
    #[cfg(target_arch = "mips")]
    {
        context.epc = context.epc + 4;
    }

    let mut syscall = Syscall {
        thread,
        #[cfg(feature = "std")]
        syscall_entry: kernel_hal_unix::syscall_entry as usize,
        #[cfg(not(feature = "std"))]
        syscall_entry: 0,
        spawn_fn: spawn,
        context,
        exit: false,
    };
    let ret = syscall.syscall(num as u32, args).await;
    let exit = syscall.exit;

    context.set_syscall_ret(ret as usize);
    exit
}
