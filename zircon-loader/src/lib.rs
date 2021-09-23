#![no_std]
#![feature(asm)]
#![deny(warnings, unused_must_use)]

#[macro_use]
extern crate alloc;
#[macro_use]
extern crate log;

use {
    alloc::{boxed::Box, sync::Arc, vec::Vec},
    core::{future::Future, pin::Pin},
    xmas_elf::ElfFile,
    zircon_object::{dev::*, ipc::*, object::*, task::*, util::elf_loader::*, vm::*},
    zircon_syscall::Syscall,
};

mod kcounter;

// These describe userboot itself
const K_PROC_SELF: usize = 0;
const K_VMARROOT_SELF: usize = 1;
// Essential job and resource handles
const K_ROOTJOB: usize = 2;
const K_ROOTRESOURCE: usize = 3;
// Essential VMO handles
const K_ZBI: usize = 4;
const K_FIRSTVDSO: usize = 5;
const K_CRASHLOG: usize = 8;
const K_COUNTERNAMES: usize = 9;
const K_COUNTERS: usize = 10;
const K_FISTINSTRUMENTATIONDATA: usize = 11;
const K_HANDLECOUNT: usize = 15;

/// Program images to run.
pub struct Images<T: AsRef<[u8]>> {
    pub userboot: T,
    pub vdso: T,
    pub zbi: T,
}

pub fn run_userboot(images: &Images<impl AsRef<[u8]>>, cmdline: &str) -> Arc<Process> {
    let job = Job::root();
    let proc = Process::create(&job, "userboot").unwrap();
    let thread = Thread::create(&proc, "userboot").unwrap();
    let resource = Resource::create(
        "root",
        ResourceKind::ROOT,
        0,
        0x1_0000_0000,
        ResourceFlags::empty(),
    );
    let vmar = proc.vmar();

    // userboot
    let (entry, userboot_size) = {
        let elf = ElfFile::new(images.userboot.as_ref()).unwrap();
        let size = elf.load_segment_size();
        let vmar = vmar
            .allocate(None, size, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
            .unwrap();
        vmar.load_from_elf(&elf).unwrap();
        (vmar.addr() + elf.header.pt2.entry_point() as usize, size)
    };

    // vdso
    let vdso_vmo = {
        let elf = ElfFile::new(images.vdso.as_ref()).unwrap();
        let vdso_vmo = VmObject::new_paged(images.vdso.as_ref().len() / PAGE_SIZE + 1);
        vdso_vmo.write(0, images.vdso.as_ref()).unwrap();
        let size = elf.load_segment_size();
        let vmar = vmar
            .allocate_at(
                userboot_size,
                size,
                VmarFlags::CAN_MAP_RXW | VmarFlags::SPECIFIC,
                PAGE_SIZE,
            )
            .unwrap();
        vmar.map_from_elf(&elf, vdso_vmo.clone()).unwrap();
        #[cfg(feature = "std")]
        {
            let offset = elf
                .get_symbol_address("zcore_syscall_entry")
                .expect("failed to locate syscall entry") as usize;
            let syscall_entry = &(kernel_hal::context::syscall_entry as usize).to_ne_bytes();
            // fill syscall entry x3
            vdso_vmo.write(offset, syscall_entry).unwrap();
            vdso_vmo.write(offset + 8, syscall_entry).unwrap();
            vdso_vmo.write(offset + 16, syscall_entry).unwrap();
        }
        vdso_vmo
    };

    // zbi
    let zbi_vmo = {
        let vmo = VmObject::new_paged(images.zbi.as_ref().len() / PAGE_SIZE + 1);
        vmo.write(0, images.zbi.as_ref()).unwrap();
        vmo.set_name("zbi");
        vmo
    };

    // stack
    const STACK_PAGES: usize = 8;
    let stack_vmo = VmObject::new_paged(STACK_PAGES);
    let flags = MMUFlags::READ | MMUFlags::WRITE | MMUFlags::USER;
    let stack_bottom = vmar
        .map(None, stack_vmo.clone(), 0, stack_vmo.len(), flags)
        .unwrap();
    #[cfg(target_arch = "x86_64")]
    // WARN: align stack to 16B, then emulate a 'call' (push rip)
    let sp = stack_bottom + stack_vmo.len() - 8;
    #[cfg(target_arch = "aarch64")]
    let sp = stack_bottom + stack_vmo.len();

    // channel
    let (user_channel, kernel_channel) = Channel::create();
    let handle = Handle::new(user_channel, Rights::DEFAULT_CHANNEL);

    let mut handles = vec![Handle::new(proc.clone(), Rights::empty()); K_HANDLECOUNT];
    handles[K_PROC_SELF] = Handle::new(proc.clone(), Rights::DEFAULT_PROCESS);
    handles[K_VMARROOT_SELF] = Handle::new(proc.vmar(), Rights::DEFAULT_VMAR | Rights::IO);
    handles[K_ROOTJOB] = Handle::new(job, Rights::DEFAULT_JOB);
    handles[K_ROOTRESOURCE] = Handle::new(resource, Rights::DEFAULT_RESOURCE);
    handles[K_ZBI] = Handle::new(zbi_vmo, Rights::DEFAULT_VMO);
    // set up handles[K_FIRSTVDSO..K_LASTVDSO + 1]
    const VDSO_DATA_CONSTANTS: usize = 0x4a50;
    const VDSO_DATA_CONSTANTS_SIZE: usize = 0x78;
    let constants: [u8; VDSO_DATA_CONSTANTS_SIZE] =
        unsafe { core::mem::transmute(kernel_hal::vdso::vdso_constants()) };
    vdso_vmo.write(VDSO_DATA_CONSTANTS, &constants).unwrap();
    vdso_vmo.set_name("vdso/full");
    let vdso_test1 = vdso_vmo.create_child(false, 0, vdso_vmo.len()).unwrap();
    vdso_test1.set_name("vdso/test1");
    let vdso_test2 = vdso_vmo.create_child(false, 0, vdso_vmo.len()).unwrap();
    vdso_test2.set_name("vdso/test2");
    handles[K_FIRSTVDSO] = Handle::new(vdso_vmo, Rights::DEFAULT_VMO | Rights::EXECUTE);
    handles[K_FIRSTVDSO + 1] = Handle::new(vdso_test1, Rights::DEFAULT_VMO | Rights::EXECUTE);
    handles[K_FIRSTVDSO + 2] = Handle::new(vdso_test2, Rights::DEFAULT_VMO | Rights::EXECUTE);
    // TODO: use correct CrashLogVmo handle
    let crash_log_vmo = VmObject::new_paged(1);
    crash_log_vmo.set_name("crashlog");
    handles[K_CRASHLOG] = Handle::new(crash_log_vmo, Rights::DEFAULT_VMO);
    let (counter_name_vmo, kcounters_vmo) = kcounter::create_kcounter_vmo();
    handles[K_COUNTERNAMES] = Handle::new(counter_name_vmo, Rights::DEFAULT_VMO);
    handles[K_COUNTERS] = Handle::new(kcounters_vmo, Rights::DEFAULT_VMO);
    // TODO: use correct Instrumentation data handle
    let instrumentation_data_vmo = VmObject::new_paged(0);
    instrumentation_data_vmo.set_name("UNIMPLEMENTED_VMO");
    handles[K_FISTINSTRUMENTATIONDATA] =
        Handle::new(instrumentation_data_vmo.clone(), Rights::DEFAULT_VMO);
    handles[K_FISTINSTRUMENTATIONDATA + 1] =
        Handle::new(instrumentation_data_vmo.clone(), Rights::DEFAULT_VMO);
    handles[K_FISTINSTRUMENTATIONDATA + 2] =
        Handle::new(instrumentation_data_vmo.clone(), Rights::DEFAULT_VMO);
    handles[K_FISTINSTRUMENTATIONDATA + 3] =
        Handle::new(instrumentation_data_vmo, Rights::DEFAULT_VMO);

    // check: handle to root proc should be only

    let data = Vec::from(cmdline.replace(':', "\0") + "\0");
    let msg = MessagePacket { data, handles };
    kernel_channel.write(msg).unwrap();

    proc.start(&thread, entry, sp, Some(handle), 0, thread_fn)
        .expect("failed to start main thread");
    proc
}

kcounter!(EXCEPTIONS_USER, "exceptions.user");
kcounter!(EXCEPTIONS_TIMER, "exceptions.timer");
kcounter!(EXCEPTIONS_PGFAULT, "exceptions.pgfault");

async fn new_thread(thread: CurrentThread) {
    kernel_hal::thread::set_tid(thread.id(), thread.proc().id());
    if thread.is_first_thread() {
        thread
            .handle_exception(ExceptionType::ProcessStarting)
            .await;
    };
    thread.handle_exception(ExceptionType::ThreadStarting).await;

    loop {
        let mut cx = thread.wait_for_run().await;
        if thread.state() == ThreadState::Dying {
            break;
        }
        trace!("go to user: {:#x?}", cx);
        debug!("switch to {}|{}", thread.proc().name(), thread.name());
        let tmp_time = kernel_hal::timer::timer_now().as_nanos();

        // * Attention
        // The code will enter a magic zone from here.
        // `context run` will be executed into a wrapped library where context switching takes place.
        // The details are available in the trapframe crate on crates.io.

        kernel_hal::context::context_run(&mut cx);

        // Back from the userspace

        let time = kernel_hal::timer::timer_now().as_nanos() - tmp_time;
        thread.time_add(time);
        trace!("back from user: {:#x?}", cx);
        EXCEPTIONS_USER.add(1);

        let trap_num = cx.trap_num;
        #[cfg(target_arch = "x86_64")]
        let error_code = cx.error_code;
        thread.end_running(cx);

        #[cfg(target_arch = "aarch64")]
        match trap_num {
            0 => handle_syscall(&thread).await,
            _ => unimplemented!(),
        }
        #[cfg(target_arch = "x86_64")]
        match trap_num {
            0x100 => handle_syscall(&thread).await,
            0x20..=0xff => {
                kernel_hal::interrupt::handle_irq(trap_num);
                // TODO: configurable
                if trap_num == 0xf1 {
                    EXCEPTIONS_TIMER.add(1);
                    kernel_hal::thread::yield_now().await;
                }
            }
            0xe => {
                EXCEPTIONS_PGFAULT.add(1);
                let (vaddr, flags) = kernel_hal::context::fetch_page_fault_info(error_code);
                info!(
                    "page fault from user mode {:#x} {:#x?} {:?}",
                    vaddr, error_code, flags
                );
                let vmar = thread.proc().vmar();
                if let Err(err) = vmar.handle_page_fault(vaddr, flags) {
                    error!("handle_page_fault error: {:?}", err);
                    thread.handle_exception(ExceptionType::FatalPageFault).await;
                }
            }
            0x8 => thread.with_context(|cx| {
                panic!("Double fault from user mode! {:#x?}", cx);
            }),
            num => {
                let type_ = match num {
                    0x1 => ExceptionType::HardwareBreakpoint,
                    0x3 => ExceptionType::SoftwareBreakpoint,
                    0x6 => ExceptionType::UndefinedInstruction,
                    0x17 => ExceptionType::UnalignedAccess,
                    _ => ExceptionType::General,
                };
                thread.handle_exception(type_).await;
            }
        }
    }
    thread.handle_exception(ExceptionType::ThreadExiting).await;
}

fn thread_fn(thread: CurrentThread) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
    Box::pin(new_thread(thread))
}

async fn handle_syscall(thread: &CurrentThread) {
    let (num, args) = thread.with_context(|cx| {
        let regs = cx.general;
        #[cfg(target_arch = "x86_64")]
        let num = regs.rax as u32;
        #[cfg(target_arch = "aarch64")]
        let num = regs.x16 as u32;
        // LibOS: Function call ABI
        #[cfg(feature = "std")]
        #[cfg(target_arch = "x86_64")]
        let args = unsafe {
            let a6 = (regs.rsp as *const usize).read();
            let a7 = (regs.rsp as *const usize).add(1).read();
            [
                regs.rdi, regs.rsi, regs.rdx, regs.rcx, regs.r8, regs.r9, a6, a7,
            ]
        };
        // RealOS: Zircon syscall ABI
        #[cfg(not(feature = "std"))]
        #[cfg(target_arch = "x86_64")]
        let args = [
            regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9, regs.r12, regs.r13,
        ];
        // ARM64
        #[cfg(target_arch = "aarch64")]
        let args = [
            regs.x0, regs.x1, regs.x2, regs.x3, regs.x4, regs.x5, regs.x6, regs.x7,
        ];
        (num, args)
    });
    let mut syscall = Syscall { thread, thread_fn };
    let ret = syscall.syscall(num, args).await as usize;
    thread.with_context(|cx| {
        #[cfg(target_arch = "x86_64")]
        {
            cx.general.rax = ret;
        }
        #[cfg(target_arch = "aarch64")]
        {
            cx.general.x0 = ret;
        }
    });
}
