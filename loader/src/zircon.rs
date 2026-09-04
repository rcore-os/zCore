//! Run Zircon user program (userboot) and manage trap/interrupt/syscall.
//!
//! Reference: <https://fuchsia.googlesource.com/fuchsia/+/3c234f79f71/zircon/kernel/lib/userabi/userboot.cc>

use alloc::{boxed::Box, format, sync::Arc, vec::Vec};
use core::{
    convert::{TryFrom, TryInto},
    future::Future,
    pin::Pin,
};

use xmas_elf::ElfFile;

use kernel_hal::context::{TrapReason, UserContext, UserContextField};
use kernel_hal::{MMUFlags, PAGE_SIZE};
use zircon_object::debuglog::DebugLog;
use zircon_object::dev::{Resource, ResourceFlags, ResourceKind};
use zircon_object::ipc::{Channel, MessagePacket};
use zircon_object::kcounter;
use zircon_object::object::{Handle, KernelObject, Rights};
use zircon_object::task::{CurrentThread, ExceptionType, Job, Process, Thread, ThreadState};
use zircon_object::util::elf_loader::{ElfExt, VmarExt};
use zircon_object::vm::{VmObject, VmarFlags};

macro_rules! include_bytes_aligned {
    ($path: expr) => {{
        #[repr(C, align(16))]
        struct Aligned<T>(T);

        static DATA: Aligned<[u8; include_bytes!($path).len()]> = Aligned(*include_bytes!($path));
        &DATA.0
    }};
}

macro_rules! boot_library {
    ($name: expr) => {{
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                boot_library!($name, "../../prebuilt/zircon/x64")
            } else if #[cfg(target_arch = "aarch64")] {
                boot_library!($name, "../../prebuilt/zircon/arm64")
            } else if #[cfg(target_arch = "riscv64")] {
                boot_library!($name, "../../prebuilt/zircon/riscv64")
            } else {
                compile_error!("Unsupported architecture for zircon mode!")
            }
        }
    }};
    ($name: expr, $base_dir: expr) => {{
        include_bytes_aligned!(concat!($base_dir, "/", $name, ".so"))
    }};
}

macro_rules! boot_vdso {
    () => {{
        #[cfg(feature = "libos")]
        {
            boot_library!("libzircon-libos")
        }
        #[cfg(not(feature = "libos"))]
        {
            boot_library!("libzircon")
        }
    }};
}

/// Run Zircon `userboot` process from the prebuilt path, and load the ZBI file as the bootfs.
pub fn run_userboot(zbi: impl AsRef<[u8]>, cmdline: &str) -> Arc<Process> {
    let mut zbi = zbi.as_ref().to_vec();
    append_core_test_filter(&mut zbi, cmdline);
    info!("Loading Zircon ZBI ({} bytes)", zbi.len());
    let test_userboot = zbi
        .windows(b"kernel.select.userboot=userboot-test-rust".len())
        .any(|window| window == b"kernel.select.userboot=userboot-test-rust");
    let userboot: &[u8] = if test_userboot {
        boot_library!("userboot-test")
    } else {
        boot_library!("userboot")
    };
    // Userboot itself is unchanged in LibOS mode.  Only the vDSO needs a
    // hosted build whose syscall stubs jump into trapframe's function-call
    // entry instead of executing the host OS `syscall` instruction.
    let vdso = boot_vdso!();

    let job = Job::root();
    job.set_name("root");
    let proc = Process::create(&job, "userboot").unwrap();
    let thread = Thread::create(&proc, "userboot").unwrap();
    let system_resource =
        Resource::create("system", ResourceKind::ROOT, 0, 16, ResourceFlags::empty());
    let vmar = proc.vmar();

    // userboot
    let (entry, userboot_size, userboot_vmar) = {
        let elf = ElfFile::new(userboot).unwrap();
        let size = elf.load_segment_size();
        let vmar = vmar
            // Modern userboot treats its own image as a non-null byte span
            // while applying static-PIE relocations.
            .allocate_at(0x10_0000, size, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
            .unwrap();
        vmar.load_from_elf(&elf).unwrap();
        (
            vmar.addr() + elf.header.pt2.entry_point() as usize,
            size,
            vmar,
        )
    };

    // vdso
    let (vdso_vmo, vdso_base) = {
        let elf = ElfFile::new(vdso).unwrap();
        let vdso_vmo = VmObject::new_paged(vdso.len() / PAGE_SIZE + 1);
        vdso_vmo.write(0, vdso).unwrap();
        const VDSO_DATA_TIME_VALUES: usize = 0x7000;
        const VDSO_DATA_CONSTANTS: usize = 0x8000;
        const VDSO_DATA_CONSTANTS_SIZE: usize = 0x78;
        let time_values: [u8; core::mem::size_of::<VdsoTimeValues>()] =
            unsafe { core::mem::transmute(vdso_time_values()) };
        vdso_vmo.write(VDSO_DATA_TIME_VALUES, &time_values).unwrap();
        let constants: [u8; VDSO_DATA_CONSTANTS_SIZE] =
            unsafe { core::mem::transmute(kernel_hal::vdso::vdso_constants()) };
        vdso_vmo.write(VDSO_DATA_CONSTANTS, &constants).unwrap();
        let size = elf.load_segment_size();
        let vmar = vmar
            .allocate_at(
                userboot_vmar.addr() - vmar.addr() + userboot_size,
                size,
                VmarFlags::CAN_MAP_RXW | VmarFlags::SPECIFIC,
                PAGE_SIZE,
            )
            .unwrap();
        vmar.map_from_elf(&elf, vdso_vmo.clone()).unwrap();
        #[cfg(feature = "libos")]
        {
            let offset = elf
                .get_symbol_address("zcore_syscall_entry")
                .expect("failed to locate syscall entry") as usize;
            let syscall_entry =
                &(kernel_hal::context::syscall_entry as *const () as usize).to_ne_bytes();
            // Fill the single shared entry used by all syscall stubs.
            vdso_vmo.write(offset, syscall_entry).unwrap();
        }
        (vdso_vmo, vmar.addr())
    };

    // zbi
    let zbi_vmo = {
        let vmo = VmObject::new_paged(zbi.len() / PAGE_SIZE + 1);
        vmo.write(0, &zbi).unwrap();
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
    let sp = if cfg!(target_arch = "x86_64") {
        // WARN: align stack to 16B, then emulate a 'call' (push rip)
        stack_bottom + stack_vmo.len() - 8
    } else {
        stack_bottom + stack_vmo.len()
    };

    // New userboot receives two handle-only bootstrap messages. Its C runtime
    // consumes the process capabilities before main, and main then consumes
    // the system capabilities.
    let (user_channel, kernel_channel) = Channel::create();
    let handle = Handle::new(user_channel, Rights::DEFAULT_CHANNEL);
    let debuglog = DebugLog::create(0);
    kernel_channel
        .write(MessagePacket {
            data: Vec::new(),
            handles: alloc::vec![
                Handle::new(debuglog.clone(), Rights::DEFAULT_DEBUGLOG),
                Handle::new(proc.clone(), Rights::DEFAULT_PROCESS),
                Handle::new(thread.clone(), Rights::DEFAULT_THREAD),
                Handle::new(proc.vmar(), Rights::DEFAULT_VMAR | Rights::IO),
                Handle::new(userboot_vmar, Rights::DEFAULT_VMAR | Rights::IO),
            ],
        })
        .unwrap();

    vdso_vmo.set_name("vdso/stable");
    kernel_channel
        .write(MessagePacket {
            data: Vec::new(),
            handles: alloc::vec![
                Handle::new(debuglog, Rights::DEFAULT_DEBUGLOG),
                Handle::new(job, Rights::DEFAULT_JOB),
                Handle::new(system_resource, Rights::DEFAULT_RESOURCE),
                Handle::new(zbi_vmo, Rights::DEFAULT_VMO),
                Handle::new(vdso_vmo, Rights::DEFAULT_VMO | Rights::EXECUTE),
            ],
        })
        .unwrap();

    let _ = cmdline;
    proc.start(&thread, entry, sp, Some(handle), vdso_base, thread_fn)
        .expect("failed to start main thread");
    proc
}

/// Append the standalone core-test filter to the in-memory ZBI command line.
///
/// Current standalone tests read gtest options directly from ZBI_TYPE_CMDLINE
/// items rather than from their process arguments.  Keep the artifact on disk
/// unchanged and add a small command-line item to the copy handed to userboot.
fn append_core_test_filter(zbi: &mut Vec<u8>, cmdline: &str) {
    const HEADER_SIZE: usize = 32;
    const ALIGNMENT: usize = 8;
    const TYPE_CMDLINE: u32 = 0x4c44_4d43;
    const FLAGS_VERSION: u32 = 1 << 16;
    const ITEM_MAGIC: u32 = 0xb578_1729;
    const ITEM_NO_CRC32: u32 = 0x4a87_e8d6;

    let Some(filter) = cmdline
        .split(':')
        .filter_map(|option| option.trim().strip_prefix("core-tests="))
        .next_back()
    else {
        return;
    };
    let payload = if filter == "-l" || filter == "--gtest_list_tests" {
        "--gtest_list_tests".into()
    } else {
        format!(
            "--gtest_filter={} --gtest_shuffle=false",
            filter.replace(',', ":")
        )
    };

    assert!(zbi.len() >= HEADER_SIZE, "invalid ZBI container");
    let container_len = u32::from_le_bytes(zbi[4..8].try_into().unwrap()) as usize;
    let item_offset = HEADER_SIZE
        .checked_add(container_len)
        .expect("ZBI container length overflow");
    assert!(item_offset <= zbi.len(), "truncated ZBI container");

    let padded_payload_len = payload.len().next_multiple_of(ALIGNMENT);
    let item_len = HEADER_SIZE + padded_payload_len;
    let new_container_len = container_len
        .checked_add(item_len)
        .and_then(|len| u32::try_from(len).ok())
        .expect("ZBI container length overflow");

    zbi.truncate(item_offset);
    zbi.resize(item_offset + item_len, 0);
    let header = [
        TYPE_CMDLINE,
        payload.len().try_into().expect("core-test filter too long"),
        0,
        FLAGS_VERSION,
        0,
        0,
        ITEM_MAGIC,
        ITEM_NO_CRC32,
    ];
    for (index, value) in header.iter().enumerate() {
        let offset = item_offset + index * core::mem::size_of::<u32>();
        zbi[offset..offset + core::mem::size_of::<u32>()].copy_from_slice(&value.to_le_bytes());
    }
    zbi[item_offset + HEADER_SIZE..item_offset + HEADER_SIZE + payload.len()]
        .copy_from_slice(payload.as_bytes());
    zbi[4..8].copy_from_slice(&new_container_len.to_le_bytes());
}

/// The unstable vDSO time ABI used by current Fuchsia. Keep this layout in
/// sync with `lib/fasttime/internal/abi.h`.
#[repr(C)]
struct VdsoTimeValues {
    version: u64,
    ticks_per_second: u64,
    boot_ticks_offset: i64,
    mono_ticks_offset: i64,
    ticks_to_time_numerator: u32,
    ticks_to_time_denominator: u32,
    usermode_can_access_ticks: u8,
    use_a73_errata_mitigation: u8,
    use_pct_instead_of_vct: u8,
    padding: [u8; 5],
}

fn vdso_time_values() -> VdsoTimeValues {
    #[cfg(any(target_arch = "x86_64", target_arch = "riscv64"))]
    let ticks_per_second = u64::from(kernel_hal::cpu::cpu_frequency()) * 1_000_000;
    #[cfg(target_arch = "aarch64")]
    let ticks_per_second = {
        let value: u64;
        unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) value) };
        value
    };

    let divisor = gcd(1_000_000_000, ticks_per_second);
    VdsoTimeValues {
        version: 1,
        ticks_per_second,
        boot_ticks_offset: 0,
        mono_ticks_offset: 0,
        ticks_to_time_numerator: (1_000_000_000 / divisor) as u32,
        ticks_to_time_denominator: (ticks_per_second / divisor) as u32,
        usermode_can_access_ticks: 1,
        use_a73_errata_mitigation: 0,
        use_pct_instead_of_vct: 0,
        padding: [0; 5],
    }
}

fn gcd(mut lhs: u64, mut rhs: u64) -> u64 {
    while rhs != 0 {
        (lhs, rhs) = (rhs, lhs % rhs);
    }
    lhs
}

kcounter!(EXCEPTIONS_USER, "exceptions.user");
kcounter!(EXCEPTIONS_IRQ, "exceptions.irq");
kcounter!(EXCEPTIONS_PGFAULT, "exceptions.pgfault");

fn thread_fn(thread: CurrentThread) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
    Box::pin(run_user(thread))
}

async fn run_user(thread: CurrentThread) {
    kernel_hal::thread::set_current_thread(Some(thread.inner()));
    if thread.is_first_thread() {
        thread
            .handle_exception(ExceptionType::ProcessStarting)
            .await;
    };
    thread.handle_exception(ExceptionType::ThreadStarting).await;

    loop {
        // wait
        let mut ctx = thread.wait_for_run().await;
        if thread.state() == ThreadState::Dying {
            break;
        }

        // run
        trace!("go to user: {:#x?}", ctx);
        debug!("switch to {}|{}", thread.proc().name(), thread.name());
        let tmp_time = kernel_hal::timer::timer_now().as_nanos();

        // * Attention
        // The code will enter a magic zone from here.
        // `enter_uspace` will be executed into a wrapped library where context switching takes place.
        // The details are available in the `trapframe` crate on crates.io.
        #[cfg(all(target_arch = "aarch64", feature = "libos"))]
        if ctx.general().x16 == 23 {
            error!(
                "aarch64 cprng enter: pc={:#x}, x18={:#x}",
                ctx.get_field(UserContextField::InstrPointer),
                ctx.general().x18,
            );
        }
        ctx.enter_uspace();

        #[cfg(all(target_arch = "aarch64", feature = "libos"))]
        if ctx.general().x16 == 23 {
            error!(
                "aarch64 cprng trap: pc={:#x}, x18={:#x}",
                ctx.get_field(UserContextField::InstrPointer),
                ctx.general().x18,
            );
        }

        // Back from the userspace
        let time = kernel_hal::timer::timer_now().as_nanos() - tmp_time;
        thread.time_add(time);
        trace!("back from user: {:#x?}", ctx);
        EXCEPTIONS_USER.add(1);

        // handle trap/interrupt/syscall
        if let Err(e) = handler_user_trap(&thread, ctx).await {
            if let ExceptionType::ThreadExiting = e {
                break;
            }
            thread.handle_exception(e).await;
        }
    }
    thread.handle_exception(ExceptionType::ThreadExiting).await;
}

async fn handler_user_trap(
    thread: &CurrentThread,
    mut ctx: Box<UserContext>,
) -> Result<(), ExceptionType> {
    let reason = ctx.trap_reason();

    if let TrapReason::Syscall = reason {
        let num = syscall_num(&ctx);
        let args = syscall_args(&ctx);
        #[cfg(all(target_arch = "aarch64", not(feature = "libos")))]
        {
            // ELR already points past SVC. Current Fuchsia vDSO stubs put a
            // 12-byte speculation barrier (DSB; ISB; BRK) after it, which the
            // Zircon syscall return path must skip.
            let ip = ctx.get_field(UserContextField::InstrPointer);
            ctx.set_field(UserContextField::InstrPointer, ip + 12);
        }
        ctx.advance_pc(reason);
        thread.put_context(ctx);
        let mut syscall = zircon_syscall::Syscall { thread, thread_fn };
        let ret = syscall.syscall(num as u32, args).await as usize;
        thread
            .with_context(|ctx| ctx.set_field(UserContextField::ReturnValue, ret))
            .map_err(|_| ExceptionType::ThreadExiting)?;
        return Ok(());
    }

    thread.put_context(ctx);
    match reason {
        TrapReason::Interrupt(vector) => {
            EXCEPTIONS_IRQ.add(1); // FIXME
            kernel_hal::interrupt::handle_irq(vector);
            kernel_hal::thread::yield_now().await;
            Ok(())
        }
        TrapReason::PageFault(vaddr, flags) => {
            EXCEPTIONS_PGFAULT.add(1);
            info!("page fault from user mode @ {:#x}({:?})", vaddr, flags);
            let vmar = thread.proc().vmar();
            vmar.handle_page_fault(vaddr, flags).map_err(|err| {
                error!(
                    "failed to handle page fault from user mode @ {:#x}({:?}): {:?}\n{:#x?}",
                    vaddr,
                    flags,
                    err,
                    thread.context_cloned()
                );
                ExceptionType::FatalPageFault
            })
        }
        TrapReason::ExtendedState => {
            thread
                .with_context(UserContext::enable_extended_state)
                .map_err(|_| ExceptionType::ThreadExiting)?;
            Ok(())
        }
        TrapReason::UndefinedInstruction => Err(ExceptionType::UndefinedInstruction),
        TrapReason::SoftwareBreakpoint => Err(ExceptionType::SoftwareBreakpoint),
        TrapReason::HardwareBreakpoint => Err(ExceptionType::HardwareBreakpoint),
        TrapReason::UnalignedAccess => Err(ExceptionType::UnalignedAccess),
        TrapReason::GernelFault(_) => Err(ExceptionType::General),
        _ => unreachable!(),
    }
}

fn syscall_num(ctx: &UserContext) -> usize {
    let regs = ctx.general();
    cfg_if! {
        if #[cfg(target_arch = "x86_64")] {
            regs.rax
        } else if #[cfg(target_arch = "aarch64")] {
            regs.x16
        } else if #[cfg(target_arch = "riscv64")] {
            regs.t0
        } else {
            unimplemented!()
        }
    }
}

fn syscall_args(ctx: &UserContext) -> [usize; 8] {
    let regs = ctx.general();
    cfg_if! {
        if #[cfg(target_arch = "x86_64")] {
            if cfg!(feature = "libos") {
                let arg7 = unsafe{ (regs.rsp as *const usize).read() };
                let arg8 = unsafe{ (regs.rsp as *const usize).add(1).read() };
                [regs.rdi, regs.rsi, regs.rdx, regs.rcx, regs.r8, regs.r9, arg7, arg8]
            } else {
                [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9, regs.r12, regs.r13]
            }
        } else if #[cfg(target_arch = "aarch64")] {
            [regs.x0, regs.x1, regs.x2, regs.x3, regs.x4, regs.x5, regs.x6, regs.x7]
        } else if #[cfg(target_arch = "riscv64")] {
            [regs.a0, regs.a1, regs.a2, regs.a3, regs.a4, regs.a5, regs.a6, regs.a7]
        } else {
            unimplemented!()
        }
    }
}
