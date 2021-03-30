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
    core::{future::Future, pin::Pin},
    kernel_hal::{UserContext, GeneralRegs, MMUFlags},
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

    {
        let mut id = 0;
        let rust_dir = rootfs.root_inode().lookup("/").unwrap();
        warn!("Rootfs: / ");
        while let Ok(name) = rust_dir.get_entry(id) {
            id += 1;
            warn!("  {}", name);
        }
    }

    let inode = rootfs.root_inode().lookup(&args[0]).unwrap();
    let data = inode.read_as_vec().unwrap();
    let path = args[0].clone();
    debug!("Linux process: {:?}", path);

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
    loop {
        // wait
        let mut cx = thread.wait_for_run().await;
        if thread.state() == ThreadState::Dying {
            break;
        }
        // run
        trace!("go to user: {:#x?}", cx);
        kernel_hal::context_run(&mut cx);
        trace!("back from user: {:#x?}", cx);
        // handle trap/interrupt/syscall

        #[cfg(target_arch = "x86_64")]
        match cx.trap_num {
            0x100 => handle_syscall(&thread, &mut cx.general).await,
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

        // UserContext
        #[cfg(target_arch = "riscv64")]
        let trap_num = kernel_hal::fetch_trap_num(&cx);
        #[cfg(target_arch = "riscv64")]
        let is_interrupt = ((trap_num >> 63) & 1) == 1;
        #[cfg(target_arch = "riscv64")]
        let trap_num = trap_num & 0xfff;

        #[cfg(target_arch = "riscv64")]
        if is_interrupt {
            match trap_num {
                //Irq
                0 | 4 | 5 | 8 => {
                    kernel_hal::InterruptManager::handle(trap_num as u8);

                    //Timer
                    if trap_num == 4 || trap_num == 5 {
                        warn!("Timer interrupt: {:#x}", trap_num);

                        kernel_hal::timer_set_next();
                        kernel_hal::timer_tick();

                        kernel_hal::yield_now().await;
                    }
                }
                _ => panic!("not supported interrupt from user mode. {:#x?}", cx),
            }

        }else{
            match trap_num {
                // syscall
                8 => handle_syscall(&thread, &mut cx).await,
                // PageFault
                12 | 13 | 15 => {
                    let vaddr = kernel_hal::fetch_fault_vaddr();
                    let flags = if trap_num == 15 {
                        MMUFlags::WRITE
                    } else {
                        MMUFlags::READ
                    };

                    info!("page fualt from user mode, vaddr:{:#x}, trap:{:#x}", vaddr, trap_num);
                    let vmar = thread.proc().vmar();
                    match vmar.handle_page_fault(vaddr, flags) {
                        Ok(()) => {}
                        Err(_) => {
                            panic!("Page Fault from user mode {:#x?}", cx);
                        }
                    }
                }
                //TODO: S-mode ext int
                _ => panic!("not supported exception from user mode. {:#x?}", cx),
            }
        }

        thread.end_running(cx);
    }
}

fn thread_fn(thread: CurrentThread) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
    Box::pin(new_thread(thread))
}

/// syscall handler entry
#[cfg(target_arch = "x86_64")]
async fn handle_syscall(thread: &CurrentThread, regs: &mut GeneralRegs) {
    trace!("syscall: {:#x?}", regs);
    let num = regs.rax as u32;
    let args = [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9];
    let mut syscall = Syscall {
        thread,
        #[cfg(feature = "std")]
        syscall_entry: kernel_hal_unix::syscall_entry as usize,
        #[cfg(not(feature = "std"))]
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
    let args = [cx.general.a0, cx.general.a1, cx.general.a2, cx.general.a3, cx.general.a4, cx.general.a5];
    // add before fork
    cx.sepc += 4;

    let regs = &mut (cx.general as GeneralRegs);
    let mut syscall = Syscall {
        thread,
        #[cfg(feature = "std")]
        syscall_entry: kernel_hal_unix::syscall_entry as usize,
        #[cfg(not(feature = "std"))]
        syscall_entry: 0,
        thread_fn,
        regs,
    };
    cx.general.a0 = syscall.syscall(num, args).await as usize;
}
