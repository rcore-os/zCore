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
    kernel_hal::{
        InterruptManager,
        // MMUFlags,
        UserContext,
    },
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

#[cfg(target_arch = "mips")]
use mips::registers::cp0;

#[cfg(target_arch = "mips")]
use mips::addr::*;

#[cfg(target_arch = "mips")]
use mips::paging::PageTable as MIPSPageTable;

#[cfg(target_arch = "mips")]
fn get_root_page_table_ptr() -> usize {
    extern "C" {
        fn _root_page_table_ptr();
    }
    unsafe { *(_root_page_table_ptr as *mut usize) }
}

#[cfg(target_arch = "mips")]
pub fn handle_user_page_fault(thread: &Arc<Thread>, addr: usize) -> bool {
    let virt_addr = VirtAddr::new(addr);
    let root_table = unsafe { &mut *(get_root_page_table_ptr() as *mut MIPSPageTable) };
    let tlb_result = root_table.lookup(addr);
    use kernel_hal::MMUFlags;
    let flags = MMUFlags::WRITE;
    match tlb_result {
        Ok(tlb_entry) => {
            trace!(
                "PhysAddr = {:x}/{:x}",
                tlb_entry.entry_lo0.get_pfn() << 12,
                tlb_entry.entry_lo1.get_pfn() << 12
            );

            let tlb_valid = if virt_addr.page_number() & 1 == 0 {
                tlb_entry.entry_lo0.valid()
            } else {
                tlb_entry.entry_lo1.valid()
            };
            if !tlb_valid {
                // if !thread.vm.lock().handle_page_fault(addr) {
                //     return false;
                // }
                match thread.proc().vmar().handle_page_fault(addr, flags) {
                    Ok(()) => {}
                    Err(_e) => {
                        return false;
                    }
                }
            }

            tlb_entry.write_random();
            true
        }
        Err(()) => match thread.proc().vmar().handle_page_fault(addr, flags) {
            Ok(()) => {
                return true;
            }
            Err(_e) => {
                return false;
            }
        },
    }
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
                _ if InterruptManager::is_page_fault(trap_num) => {
                    let addr = cp0::bad_vaddr::read_u32() as usize;
                    if !handle_user_page_fault(&thread, addr) {
                        // TODO: SIGSEGV
                        panic!("page fault handle failed");
                    }
                }
                _ if InterruptManager::is_syscall(trap_num) => {
                    exit = handle_syscall(&thread, &mut cx).await
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

async fn handle_syscall(thread: &Arc<Thread>, context: &mut UserContext) -> bool {
    let regs = &context.general;
    trace!("syscall: {:#x?}", regs);
    let num = context.get_syscall_num();
    let args = context.get_syscall_args();

    // add before fork
    #[cfg(riscv)]
    {
        context.sepc = context.sepc + 4;
    }
    #[cfg(mipsel)]
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
