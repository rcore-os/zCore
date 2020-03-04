#![no_std]
#![feature(asm)]
#![feature(global_asm)]
#![deny(warnings, unused_must_use)]

extern crate alloc;
#[macro_use]
extern crate log;

use {
    alloc::{boxed::Box, string::String, sync::Arc, vec::Vec},
    kernel_hal::GeneralRegs,
    linux_object::{
        fs::{vfs::FileSystem, INodeExt},
        loader::LinuxElfLoader,
        process::ProcessExt,
        thread::ThreadExt,
    },
    linux_syscall::Syscall,
    zircon_object::task::*,
};

pub fn run(
    exec_path: &str,
    args: Vec<String>,
    envs: Vec<String>,
    rootfs: Arc<dyn FileSystem>,
) -> Arc<Process> {
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
    let inode = rootfs.root_inode().lookup(&exec_path).unwrap();
    let data = inode.read_as_vec().unwrap();
    let (entry, sp) = loader.load(&proc.vmar(), &data, args, envs).unwrap();

    thread
        .start(entry, sp, 0, 0)
        .expect("failed to start main thread");
    proc
}

#[no_mangle]
pub fn run_task(thread: Arc<Thread>) {
    let vmtoken = thread.proc().vmar().table_phys();
    let future = async move {
        loop {
            let mut cx = thread.wait_for_run().await;
            trace!("go to user: {:#x?}", cx);
            kernel_hal::context_run(&mut cx);
            trace!("back from user: {:#x?}", cx);
            assert_eq!(cx.trap_num, 0x100, "user interrupt still no support");
            let exit = handle_syscall(&thread, &mut cx.general).await;
            thread.end_running(cx);
            if exit {
                break;
            }
        }
    };
    kernel_hal::Thread::spawn(Box::pin(future), vmtoken);
}

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
        regs,
        exit: false,
    };
    let ret = syscall.syscall(num, args).await;
    let exit = syscall.exit;
    regs.rax = ret as usize;
    exit
}
