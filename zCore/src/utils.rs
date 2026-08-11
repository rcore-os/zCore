use alloc::{collections::BTreeMap, string::String, sync::Arc};
use zircon_object::{object::KernelObject, task::Process};

#[derive(Debug)]
#[allow(dead_code)]
pub struct BootOptions {
    pub cmdline: String,
    pub log_level: String,
    /// Process run as PID 1 / init (`INIT`), e.g. `/sbin/init`. Empty
    /// or a missing binary means the system boots without a PID 1 init.
    #[cfg(feature = "linux")]
    pub init_proc: String,
    /// Process run on every terminal shell (`SHELL`), with PIDs 101.. — empty
    /// means no terminal shells (e.g. libos, where `INIT` is the one program).
    #[cfg(feature = "linux")]
    pub shell_proc: String,
}

fn parse_cmdline(cmdline: &str) -> BTreeMap<&str, &str> {
    let mut options = BTreeMap::new();
    for opt in cmdline.split(':') {
        // parse "key=value"
        let mut iter = opt.trim().splitn(2, '=');
        if let Some(key) = iter.next() {
            if let Some(value) = iter.next() {
                options.insert(key.trim(), value.trim());
            }
        }
    }
    options
}

pub fn boot_options() -> BootOptions {
    cfg_if! {
        if #[cfg(feature = "libos")] {
            let args = std::env::args().collect::<Vec<_>>();
            if args.len() < 2 {
                #[cfg(feature = "linux")]
                println!("Usage: {} PROGRAM", args[0]);
                #[cfg(feature = "zircon")]
                println!("Usage: {} ZBI_FILE [CMDLINE]", args[0]);
                std::process::exit(-1);
            }

            let (cmdline, log_level) = if cfg!(feature = "zircon") {
                let cmdline = args.get(2).cloned().unwrap_or_default();
                let options = parse_cmdline(&cmdline);
                let log_level = String::from(*options.get("LOG").unwrap_or(&""));
                (cmdline, log_level)
            } else {
                (String::new(), std::env::var("LOG").unwrap_or_default())
            };
            BootOptions {
                cmdline,
                log_level,
                // libos runs a single program: it is PID 1 (init), no shells.
                #[cfg(feature = "linux")]
                init_proc: args[1..].join("?"),
                #[cfg(feature = "linux")]
                shell_proc: String::new(),
            }
        } else {
            use alloc::string::ToString;
            let cmdline = kernel_hal::boot::cmdline();
            let options = parse_cmdline(&cmdline);
            BootOptions {
                cmdline: cmdline.clone(),
                log_level: options.get("LOG").unwrap_or(&"").to_string(),
                // `INIT` selects the PID 1 process. Default `/sbin/init`, which
                // the rootfs points at Eclipse's native `eclipse-init` (the
                // default init system); if its cross-build was unavailable the
                // same `/sbin/init` symlink falls back to busybox's `init` applet.
                // Run only if it exists. `SHELL` selects the per-terminal shells
                // at PIDs 101.. (default busybox); `ROOTPROC` is accepted as a
                // deprecated alias for `SHELL`.
                #[cfg(feature = "linux")]
                init_proc: options.get("INIT").unwrap_or(&"/sbin/init").to_string(),
                #[cfg(feature = "linux")]
                shell_proc: options
                    .get("SHELL")
                    .or_else(|| options.get("ROOTPROC"))
                    .unwrap_or(&"/bin/busybox?sh")
                    .to_string(),
            }
        }
    }
}

fn check_exit_code(proc: Arc<Process>) -> i32 {
    let code = proc.exit_code().unwrap_or(-1);
    if code != 0 {
        error!(
            "process {:?}({}) exited with code {:?}",
            proc.name(),
            proc.id(),
            code
        );
    } else {
        info!(
            "process {:?}({}) exited with code 0",
            proc.name(),
            proc.id()
        )
    }
    code as i32
}

#[cfg(feature = "libos")]
pub fn wait_for_exit(proc: Option<Arc<Process>>) -> ! {
    let exit_code = if let Some(proc) = proc {
        let future = async move {
            use zircon_object::object::Signal;
            let object: Arc<dyn KernelObject> = proc.clone();
            let signal = if cfg!(any(feature = "linux", feature = "baremetal-test")) {
                Signal::PROCESS_TERMINATED
            } else {
                Signal::USER_SIGNAL_0
            };
            object.wait_signal(signal).await;
            check_exit_code(proc)
        };

        // If the graphic mode is on, run the process in another thread.
        #[cfg(feature = "graphic")]
        let future = {
            let handle = async_std::task::spawn(future);
            kernel_hal::libos::run_graphic_service();
            handle
        };

        async_std::task::block_on(future)
    } else {
        warn!("No process to run, exit!");
        0
    };
    std::process::exit(exit_code);
}

#[cfg(not(feature = "libos"))]
pub fn wait_for_exit(proc: Option<Arc<Process>>) -> ! {
    kernel_hal::timer::timer_enable();
    // Executors call this when their CPU runs out of tasks, right before
    // halting: drain NIC/driver work pushed from IRQ context and flush stdin
    // data whose EventBus notification was deferred (try_lock failed), so the
    // wakers fire now instead of waiting for a thread to re-enter the net
    // stack or for the next timer tick. Returning `true` makes the executor
    // re-check its run queue instead of halting.
    executor::set_idle_callback(|| {
        // Lazy-TLB restore point: this CPU is about to idle (or steal work),
        // so drop any lingering user CR3 and return to the kernel page table.
        // `ThreadSwitchFuture::poll` no longer restores the kernel CR3 after
        // every poll (that TLB flush dominated yield/syscall latency); it is
        // restored here instead, before the CPU can halt with a user CR3 that
        // a concurrent process exit might later free.
        kernel_hal::vm::activate_kernel_paging();
        let had_jobs = kernel_hal::deferred_job::pending_deferred_jobs() > 0;
        // Record the idle-callback hit rate so `/proc/perf/kernel` can show
        // whether the CPUs keep finding deferred work (busy-spin = heat) or
        // actually reach the halt below.
        kernel_hal::kstats::note_idle_callback(had_jobs);
        if had_jobs {
            kernel_hal::deferred_job::drain_deferred_jobs();
        }
        #[cfg(feature = "linux")]
        {
            use linux_object::fs::stdio::STDIN;
            STDIN.flush_ready_flag();
        }
        if !had_jobs {
            // No work and about to halt: stretch this CPU's scheduler tick to
            // the next pending timer (capped) so an idle CPU stops taking the
            // full 250 Hz tick. `timer_idle_exit` (on the next poll) restores it.
            kernel_hal::timer::timer_idle_enter();
        }
        had_jobs
    });
    info!("executor run!");
    // Build this CPU's lazily-constructed runtime (multi-MiB executor stack,
    // guard-page unmaps with their TLB shootdowns) BEFORE announcing IPI
    // readiness below: while constructing, this CPU cannot ack peers'
    // shootdowns, so it must not yet be a target — the same regime the old
    // eager warm-up had, where the BSP built every runtime while APs were
    // still not IPI-ready. No-op on the BSP (built in `primary_init`) and on
    // re-entry.
    executor::warm_runtimes();
    // This CPU is now entering the executor loop, where it runs with interrupts
    // enabled and will service TLB-shootdown IPIs. Announce it as a valid
    // shootdown target: until now an AP spins on `STARTED` with IRQs off and
    // could not ack, so a shootdown that waited on it would stall.
    kernel_hal::mark_cpu_ipi_ready(kernel_hal::cpu::cpu_id() as usize);
    loop {
        // In normal builds `run_until_idle` never returns (idle work happens in
        // the callback above); it only returns under `baremetal-test` when the
        // task queue is empty.
        let has_task = executor::run_until_idle();
        if !has_task && cfg!(feature = "baremetal-test") {
            proc.map(check_exit_code);
            kernel_hal::cpu::reset();
        }
        kernel_hal::interrupt::wait_for_interrupt();
    }
}

#[cfg(all(not(feature = "libos"), feature = "mock-disk"))]
pub fn mock_disk() -> ! {
    use crate::fs::init_ram_disk;
    info!("mock core: {}", kernel_hal::cpu::cpu_id());
    if let Some(initrd) = init_ram_disk() {
        linux_object::fs::mocking_block(initrd)
    } else {
        panic!("can't find disk image in memory")
    }
}

// pub fn nvme_test(){
//     use alloc::boxed::Box;
//     let irq = kernel_hal::drivers::all_irq().find("riscv-plic").unwrap();
//     let nvme = kernel_hal::drivers::all_block().find("nvme").unwrap();
//     let irq_num = 33;
//     let _r = irq.register_handler(irq_num, Box::new(move || nvme.handle_irq(irq_num)));

//     let _r = irq.unmask(irq_num);

//     let nvme_block = kernel_hal::drivers::all_block()
//     .find("nvme")
//     .unwrap();

//     let buf1:&[u8] = &[1u8;512];
//     let _r = nvme_block.write_block(0, &buf1);
//     warn!("r {:?}", _r);
//     let mut read_buf = [0u8; 512];
//     let _r = nvme_block.read_block(0, &mut read_buf);
//     warn!("read_buf: {:?}", read_buf);

//     let buf2:&[u8] = &[2u8;512];
//     let _r = nvme_block.write_block(1, &buf2);
//     warn!("r {:?}", _r);
//     let mut read_buf = [0u8; 512];
//     let _r = nvme_block.read_block(1, &mut read_buf);
//     warn!("read_buf: {:?}", read_buf);
// }
