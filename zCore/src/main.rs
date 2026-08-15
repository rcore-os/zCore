#![cfg_attr(not(feature = "libos"), no_std)]
#![cfg_attr(not(feature = "libos"), feature(alloc_error_handler))]
#![deny(warnings)]
#![no_main]

use core::sync::atomic::{AtomicBool, Ordering};

extern crate alloc;
#[macro_use]
extern crate log;
#[macro_use]
extern crate cfg_if;

#[macro_use]
mod logging;

#[cfg(not(feature = "libos"))]
mod lang;
#[cfg(not(feature = "libos"))]
mod oops;

mod fs;
mod handler;
mod invariants;
mod platform;
mod utils;

cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        #[path = "memory_x86_64.rs"]
        mod memory;
    } else {
        mod memory;
    }
}

static STARTED: AtomicBool = AtomicBool::new(false);

#[cfg(all(not(any(feature = "libos")), feature = "mock-disk"))]
static MOCK_CORE: AtomicBool = AtomicBool::new(false);

fn primary_main(config: kernel_hal::KernelConfig) {
    logging::init();
    memory::init();
    kernel_hal::primary_init_early(config, &handler::ZcoreKernelHandler);
    kernel_hal::console::early_progress_bar(55);
    kernel_hal::console::early_progress_bar(60);
    let options = utils::boot_options();
    logging::set_max_level(&options.log_level);
    kernel_hal::console::early_progress_bar(70);
    #[cfg(feature = "linux")]
    let (init_proc, shell_proc): (&str, &str) = (&options.init_proc, &options.shell_proc);
    #[cfg(not(feature = "linux"))]
    let (init_proc, shell_proc) = ("N/A", "N/A");

    klog_info!(
        "Eclipse: boot options log_level={} init={} shell={}",
        options.log_level,
        init_proc,
        shell_proc
    );
    // Diagnostic-build fingerprint. Bumped by hand on every commit that changes
    // what a crash log looks like, and printed unconditionally, so a pasted log
    // says WHICH kernel produced it — two hunts have already stalled on "did
    // this boot actually include the new detector, or is it last week's build?".
    klog_info!("Eclipse: diag rev 10 (isolation-first containment: kill the culprit, keep the kernel up)");
    // Scheduler/timer A-B switches. Both default on; `TIMERDEADLINE=0` and
    // `WAKEPREEMPT=0` on the kernel command line restore the pre-change
    // behaviour. They exist so a suspected regression can be bisected on ONE
    // build by rebooting, instead of by rebuilding — a rebuild changes the
    // binary and its layout, and a QEMU/TCG run has enough variance to hide a
    // real difference either way. See docs/README-performance.md.
    #[cfg(not(feature = "libos"))]
    {
        if options.cmdline.contains("TIMERDEADLINE=0") {
            kernel_hal::timer::set_deadline_timer(false);
            klog_info!("Eclipse: deadline timer DISABLED (TIMERDEADLINE=0)");
        }
        if options.cmdline.contains("WAKEPREEMPT=0") {
            executor::set_wakeup_preempt(false);
            klog_info!("Eclipse: wake-up preemption DISABLED (WAKEPREEMPT=0)");
        }
        // Kernel fault containment (see `oops`). ON by default: a fault taken
        // while serving a process kills that process and the system carries on.
        // `PANICONOOPS=1` restores the immediate halt, which is what one wants
        // while debugging a specific failure — the machine freezes on the spot,
        // dump intact and the guilty coroutine still standing.
        if options.cmdline.contains("PANICONOOPS=1") {
            oops::set_panic_on_oops(true);
            klog_info!("Eclipse: kernel fault containment DISABLED (PANICONOOPS=1)");
        } else {
            klog_info!(
                "Eclipse: kernel fault containment ENABLED -- a kernel fault while \
                 serving a process kills that process, not the system \
                 (disable with PANICONOOPS=1)"
            );
        }
        // `STACKQUARANTINE=1` holds every freed coroutine stack write-protected
        // in a bounded ring instead of returning it to the heap, so a dangling
        // pointer that writes into freed stack memory faults AT THE WRITER's rip
        // (`[stack-uaf]`) — the diagnostic that pins the transient-executor UAF.
        // Costs held memory + a TLB shootdown per free, so it is opt-in.
        if options.cmdline.contains("STACKQUARANTINE=1") {
            executor::set_stack_quarantine_enabled(true);
            klog_info!(
                "Eclipse: freed-stack quarantine ENABLED (STACKQUARANTINE=1) — \
                 catching the use-after-free writer at its rip"
            );
        }
        // Deadlock-detector sensitivity. The default threshold is a spin COUNT
        // calibrated for real hardware (~8 s); under QEMU/TCG the guest runs
        // orders of magnitude slower, so that count takes many minutes and the
        // detector never fires within a test run — which is how a genuine hang
        // came to be misread as "not a deadlock, nothing was reported". Lower it
        // for emulated runs: `DEADLOCKSPINS=20000000`.
        if let Some(rest) = options.cmdline.split("DEADLOCKSPINS=").nth(1) {
            let digits: alloc::string::String =
                rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u64>() {
                lock::set_deadlock_spins(n);
                klog_info!("Eclipse: deadlock spin threshold set to {}", n);
            }
        }
        // Default is ON (see `COW_FORK`): the user-memory corruption that had it
        // rolled back was a stale writable TLB entry after `protect_for_cow`, now
        // fixed by the shootdown spin-pump — root cause and re-validation recorded
        // there. `FORKCOW=0` is the kill-switch; `FORKCOW=1` stays accepted so
        // existing command lines keep meaning what they say.
        if options.cmdline.contains("FORKCOW=1") {
            zircon_object::vm::set_cow_fork(true);
            klog_info!("Eclipse: copy-on-write fork ENABLED (FORKCOW=1)");
        }
        if options.cmdline.contains("FORKCOW=0") {
            zircon_object::vm::set_cow_fork(false);
            klog_info!("Eclipse: copy-on-write fork DISABLED (FORKCOW=0) -- eager copy");
        }
        // Time every kernel heap allocation and free. Off by default because
        // this is the hottest path in the kernel; on, it is two `rdtsc`s and two
        // relaxed adds per allocation, which is enough to answer whether the
        // allocator is what makes `fork` quadratic in its mapping count.
        if options.cmdline.contains("HEAPPROF=1") {
            kernel_hal::kstats::set_heap_prof(true);
            klog_info!("Eclipse: kernel heap profiling ENABLED (HEAPPROF=1)");
        }
        // Boot-time file-access recorder. `BOOTTRACE=<comm>` arms it for the one
        // process whose comm matches (e.g. BOOTTRACE=labwc): every file it opens
        // is logged with a timestamp and result, surfaced at /proc/bootprofile
        // as a startup timeline plus a deduplicated preload list. Off by default
        // — one relaxed atomic load per openat when unset — so this is a
        // measurement build knob, not a permanent cost. The comm runs to the
        // next space/comma (labwc's comm has neither).
        if let Some(rest) = options.cmdline.split("BOOTTRACE=").nth(1) {
            // Stop at any cmdline separator: ':' (the QEMU-style list), a space,
            // or ','. A comm has none of these.
            let comm: alloc::string::String = rest
                .chars()
                .take_while(|c| !c.is_ascii_whitespace() && *c != ',' && *c != ':')
                .collect();
            if !comm.is_empty() {
                linux_object::boot_trace::set_target(&comm);
                klog_info!("Eclipse: boot-trace ARMED for comm '{}' (BOOTTRACE)", comm);
            }
        }
        // Hunter's flood/fork-bomb rate heuristics. OFF by default: they cost a
        // clock read + an IRQ-off shard-lock + BTreeMap update on EVERY syscall
        // of every process (all threads of one process serialize on the shard).
        // The constant-time sensitive-syscall WATCH stays on regardless — only
        // the rate counters are opt-in. Two spellings accepted: HUNTERANOMALY=1
        // and HUNTER_ANOMALY=1 (a parallel branch introduced the underscored
        // A/B knob when the default was still on; =0 is now the default state).
        if options.cmdline.contains("HUNTERANOMALY=1")
            || options.cmdline.contains("HUNTER_ANOMALY=1")
        {
            hunter::heuristics::set_anomaly_detection(true);
            klog_info!("Eclipse: hunter rate heuristics ENABLED (HUNTERANOMALY=1)");
        }
        // Batching a fork's cross-CPU TLB shootdowns into one. Default on, and
        // only ever applied when the forking process has a single thread — see
        // `VmAddressRegion::fork_from` for why that condition is what makes it
        // sound. `FORKGATHER=0` restores one shootdown per mapping, which is
        // what the benchmark's `fork cost per mapping` row exists to expose.
        if options.cmdline.contains("FORKGATHER=0") {
            zircon_object::vm::set_fork_gather(false);
            klog_info!("Eclipse: batched fork TLB shootdown DISABLED (FORKGATHER=0)");
        }
        // The vDSO answers `clock_gettime` from the TSC in userspace, which
        // requires the counter to be invariant: constant-rate, and
        // reset-synchronized across cores. That is read from
        // CPUID.80000007H:EDX[8], which every x86 since about 2008 sets — and
        // which QEMU cannot set under TCG at all, because the feature word is
        // not in its TCG-supported set. `+invtsc` on the command line is
        // silently dropped there.
        //
        // So on an emulator the vDSO would sit permanently disabled and could
        // never be measured, on the one substrate where Eclipse and Linux can
        // be compared fairly. `VDSOFORCE=1` says "trust the TSC anyway". It is
        // sound under TCG, where every vCPU's `rdtsc` derives from a single
        // host clock and is therefore synchronized by construction — and it is
        // NOT a switch to set on real hardware that declines to advertise an
        // invariant TSC, because there the counter may genuinely drift between
        // sockets and userspace has no way to notice.
        if options.cmdline.contains("VDSOFORCE=1") {
            kernel_hal::timer::set_force_tsc_invariant(true);
            klog_info!("Eclipse: vDSO forced on despite CPUID (VDSOFORCE=1)");
        }
        // SMP master switch. Default OFF: boot single-core unless `smp=on` is on
        // the cmdline. Multi-core bring-up currently wedges some real hardware at
        // the scheduler hand-off (100% boot, no further progress), while
        // single-core is solid; defaulting off keeps a stock image always
        // bootable until that hang is root-caused. `smp=on` re-enables AP
        // bring-up (QEMU, or a machine known to survive multi-core / for
        // debugging the hang with a serial log).
        if options.cmdline.contains("smp=on") {
            kernel_hal::set_smp_enabled(true);
            klog_info!("Eclipse: SMP multi-core bring-up ENABLED (smp=on)");
        } else {
            klog_info!(
                "Eclipse: single-core (SMP off by default — pass `smp=on` to enable multi-core)"
            );
        }
    }
    // Deadlock self-report: any CPU spinning >~8s on a kernel spinlock paints
    // the stuck call site(s) onto the red framebuffer banner (lock-free), so a
    // silent freeze names its own deadlock instead of needing a serial cable.
    // Kept permanently -- it is free until something actually deadlocks.
    #[cfg(not(feature = "libos"))]
    lock::set_deadlock_hook(lang::deadlock_report);
    // A CPU spinning on a kernel lock has IRQs off and cannot ack TLB
    // shootdowns; the shootdown initiator spin-waits for that ack. The pump
    // lets spinners drain their own shootdown queue mid-spin (lock-free work
    // only), breaking the convoy that made parallel VM operations measure 80x
    // slower in absolute terms than single-threaded.
    #[cfg(not(feature = "libos"))]
    lock::set_spin_pump(kernel_hal::tlb_shootdown_pump);
    // Second hook: alongside the stuck WAITERS, paint the acquire site of the
    // lock's current HOLDER (snapshotted from the lock by kernel-sync). The
    // waiters in the banner are usually innocent readers; the HOLDER line is
    // the one that names the wedged code path.
    #[cfg(not(feature = "libos"))]
    lock::set_deadlock_holder_hook(lang::deadlock_holder_report);
    // NOTE: present-over-graphics diagnostic is now OFF (the lazy fork map
    // fixed the stall it was hunting) -- labwc owns the screen again in
    // KD_GRAPHICS; kernel logs go to dmesg and the text console only.
    memory::insert_regions(&kernel_hal::mem::free_pmem_regions());
    // The kernel runs on the BOOTLOADER's page tables, which live in memory
    // UEFI reports as reclaimable — the line above just fed those live
    // page-table frames to the allocator as free RAM. Pull them back out
    // before the first allocation can recycle one (see the function's doc for
    // the corruption/triple-fault chain this caused).
    #[cfg(target_arch = "x86_64")]
    memory::reserve_active_page_table_frames();
    kernel_hal::console::early_progress_bar(80);
    kernel_hal::primary_init();
    // Bring up the hunter security subsystem. Register the monotonic clock
    // first so the very first recorded event carries a real timestamp, then a
    // durable sink that streams high-severity (Warning+) events to the kernel
    // log before any in-memory ring eviction can erase them, then initialize
    // the policy/IDS engine.
    hunter::set_time_source(|| kernel_hal::timer::timer_now().as_nanos() as u64);
    hunter::set_sink(|e: &hunter::event_log::LogEntry| {
        kernel_hal::klog_info!(
            "hunter[{}] {} {} pid={} {}",
            e.severity.as_str(),
            e.category,
            e.action,
            e.pid,
            e.description
        );
    });
    hunter::init();
    // (The anomaly-rate heuristics now default OFF — see the HUNTERANOMALY=1 /
    // HUNTER_ANOMALY=1 opt-in in the cmdline-switch block above. The old
    // HUNTER_ANOMALY=0 A/B knob became the default state.)
    kernel_hal::console::early_progress_bar(90);
    cfg_if! {
        if #[cfg(all(feature = "linux", feature = "zircon"))] {
            panic!("Feature `linux` and `zircon` cannot be enabled at the same time!");
        } else if #[cfg(feature = "linux")] {
            use linux_object::process::ProcessExt;
            use zircon_object::object::KernelObject;
            // Parse "arg0?arg1?arg2"; an empty string yields no program.
            fn parse_proc(s: &str) -> alloc::vec::Vec<alloc::string::String> {
                if s.is_empty() {
                    alloc::vec::Vec::new()
                } else {
                    s.split('?').map(Into::into).collect()
                }
            }
            let init_args = parse_proc(&options.init_proc);
            let shell_args = parse_proc(&options.shell_proc);
            // Base environment for the shells and PID 1 init. `HOME`/`TERM`/
            // `USER`/`LOGNAME` are set HERE (not just in /etc/profile) because
            // bash, unlike POSIX sh, ignores `ENV` and only sources /etc/profile
            // as a *login* shell — without these in the real environment bash
            // greets with "I can't find my home directory!" and readline (tab
            // completion) misbehaves for lack of `TERM`.
            let envs: alloc::vec::Vec<alloc::string::String> = alloc::vec![
                // /usr/local/bin first so the Eclipse labwc wrapper (which
                // forces the pixman renderer) shadows the apk-installed binary.
                "PATH=/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
                "ENV=/etc/profile".into(),
                "HOME=/root".into(),
                "TERM=xterm-256color".into(),
                "USER=root".into(),
                "LOGNAME=root".into(),
                // UTF-8 locale so ncurses/readline use Unicode box-drawing and
                // compute character widths correctly (the console renders the
                // box-drawing/block code points procedurally).
                "LANG=C.UTF-8".into(),
                "LC_ALL=C.UTF-8".into(),
                // wlroots/labwc configuration. Set in the real environment (not
                // only /etc/profile) so a compositor launched from a *non-login*
                // shell — which busybox sh does not source /etc/profile for —
                // still gets them. Without WLR_RENDERER=pixman labwc tries
                // GLES2/EGL then Vulkan; with no GPU that path may fail outright,
                // or run on Mesa llvmpipe and be extremely slow because each
                // frame is rendered and copied on CPU. See xtask write_profile
                // for the rationale of each.
                //"WLR_RENDERER=pixman".into(),
                //"WLR_RENDERER_ALLOW_SOFTWARE=1".into(),
                // WLR_NO_HARDWARE_CURSORS deliberately NOT set: the kernel DRM
                // scheme composites the legacy MODE_CURSOR bitmap over every
                // scanned-out frame (drm.rs set_cursor_bo/move_cursor), so
                // wlroots' hardware-cursor path both works and avoids
                // re-rendering the whole scene on pointer moves.
                //"WLR_LIBINPUT_NO_DEVICES=1".into(),
                // On systems with multiple GPUs (e.g. two NVIDIA RTX 2060
                // SUPER cards) wlroots enumerates ALL DRM nodes unless this
                // variable restricts it. Opening card1 (compute GPU, no UEFI
                // framebuffer) in addition to card0 causes two problems:
                //   1. Both cards share the same global DRM_STATE and expose
                //      identical synthetic CRTC/connector IDs → wlroots
                //      allocates 0×0 dumb buffers for the "second" output.
                //   2. The NVIDIA stub DRM node has no real GLES2/GBM support;
                //      the gles2/GBM path hangs the whole OS at GL FBO
                //      creation (see xtask write_labwc_config comment).
                // Restricting to card0 (the console GPU that has the UEFI
                // framebuffer) prevents both issues. Also set in /etc/profile
                // and the /usr/local/bin/labwc wrapper, but only this kernel-
                // env entry is visible to labwc when launched from a non-login
                // shell or directly from an init script.
                "WLR_DRM_DEVICES=/dev/dri/card0".into(),
                // Software GL/Vulkan via Mesa (no usable HW 3D engine here). The
                // DRM device now advertises a real NVIDIA PCI id, so Mesa would
                // otherwise try to load the hardware `nouveau` driver (whose
                // ioctls we don't implement) and fail/hang. Force the KMS
                // software rasteriser (kms_swrast → llvmpipe) instead, which
                // renders into dumb buffers — exactly our DRM capabilities.
                // Only takes effect when a GL renderer is selected
                // (WLR_RENDERER=gles2); the pixman default ignores Mesa because
                // the llvmpipe GL path is much slower here.
                //"GALLIUM_DRIVER=llvmpipe".into(),
                //"MESA_LOADER_DRIVER_OVERRIDE=kms_swrast".into(),
            ];
            let rootfs = fs::rootfs();
            // Load hunter's /etc/hunter/{whitelist,blacklist} from the root fs
            // and enable exec learning (trust-on-first-use). Safe if absent.
            linux_object::fs::hunter_config::load(&rootfs.root_inode());
            // Real NVIDIA GSP-RM firmware (see xtask's nvidia_firmware and
            // nvidia-rm-sys/vendor/eclipse_rm_init.c): the display driver
            // runs during early PCI enumeration, before any filesystem
            // exists, so it can't read this itself -- pushed down here,
            // right after rootfs mount, to every registered DRM driver
            // (only the real NVIDIA one does anything with it). Missing
            // file (fetch failed at image-build time, or a non-NVIDIA
            // build) is a normal, silent no-op.
            load_nvidia_gsp_firmware(&rootfs.root_inode());
            // Auto bring-up every COMPUTE GPU (any NVIDIA GPU not driving the
            // boot display) now that the GSP firmware is available, so the
            // copy-engine present path (ce_present over PCIe P2P) is ready
            // before the compositor starts -- no manual `cat /proc/gpustep5;6;8;9`.
            // Runs once, synchronously, before any userspace/scanout touches RM.
            // Kill switch: boot with `nvidia.noautoboot` on the kernel cmdline.
            if !options.cmdline.contains("nvidia.noautoboot") {
                auto_bringup_compute_gpus();
            } else {
                klog_info!("Eclipse: NVIDIA compute-GPU auto bring-up disabled (nvidia.noautoboot)");
            }
            // Per-frame CE-offloaded present is strictly OPT-IN: it fires a GPU
            // DMA on every desktop present and, on real hardware, correlated
            // with random SIGSEGVs (139) in freshly-started Wayland clients
            // while the CPU blit was rock solid. Boot with `nvidia.cepresent`
            // to enable it for testing; the default present is the CPU blit
            // even when the compute GPU was auto-brought-up above (bring-up
            // stays useful for /proc/gpustep* work and future acceleration).
            if options.cmdline.contains("nvidia.cepresent") {
                linux_object::fs::devfs::drm::set_ce_present_enabled(true);
                klog_info!("Eclipse: NVIDIA CE-offload present ENABLED (nvidia.cepresent)");
            } else {
                klog_info!("Eclipse: present por CPU (CE-offload opt-in: nvidia.cepresent)");
            }
            // Atomic modesetting uAPI (DRM_CLIENT_CAP_ATOMIC +
            // DRM_IOCTL_MODE_ATOMIC) is OPT-IN while the legacy-KMS path
            // remains the one proven on real hardware — same rollout nouveau
            // used (`nouveau.atomic=1`). Boot with `drm.atomic` to let
            // compositors take the atomic path; without it they fall back to
            // legacy KMS exactly as before.
            if options.cmdline.contains("drm.atomic") {
                linux_object::fs::devfs::drm::set_atomic_enabled(true);
                klog_info!("Eclipse: DRM atomic modesetting ENABLED (drm.atomic)");
            }
            // Nouveau-compatible driver-specific ioctl surface on the NVIDIA
            // DRM node (GETPARAM, CHANNEL_ALLOC, GEM_NEW/INFO, VM_INIT --
            // see drivers/src/display/nouveau_uapi.rs and
            // docs/README-nouveau-uapi.md for exactly what is and isn't
            // implemented yet). Off by default: untested against real
            // hardware in the sandbox that wrote it.
            //
            // The flag is a REQUEST; `nouveau_uapi_capable()` is the
            // CAPABILITY, and only `NvidiaGpu` reports it. Both must hold:
            // `GL=1` stamps `renderer=gl:nvidia.nouveau_uapi` into one cmdline
            // that is booted on the RTX box AND under QEMU, and honouring the
            // flag with no NVIDIA GPU registered would make QEMU's virtio-gpu
            // node answer `DRM_IOCTL_VERSION` with the name "nouveau" (so Mesa
            // loads NVK and aims the whole nouveau uAPI at a driver that
            // implements none of it), advertise `DRM_CAP_SYNCOBJ`/`_TIMELINE`,
            // and open the syncobj ioctl paths -- all of it on the software
            // desktop that is supposed to keep working. Same image, both
            // machines, and each gets only what its hardware can serve.
            if options.cmdline.contains("nvidia.nouveau_uapi") {
                let capable = kernel_hal::drivers::all_drm()
                    .as_vec()
                    .iter()
                    .any(|d| d.nouveau_uapi_capable());
                if capable {
                    kernel_hal::drivers::set_nouveau_uapi_enabled(true);
                    klog_info!(
                        "Eclipse: nouveau-compatible uAPI ENABLED (nvidia.nouveau_uapi) -- see docs/README-nouveau-uapi.md"
                    );
                } else {
                    klog_info!(
                        "Eclipse: nvidia.nouveau_uapi pedido pero NO hay ninguna GPU NVIDIA registrada -- uAPI nouveau DESACTIVADA (el nodo DRM sigue identificandose como \"zcore\")"
                    );
                }
            }
            kernel_hal::console::early_progress_bar(95);

            // Whose exit takes the system down: INIT (PID 1) if present, else
            // the primary terminal's shell.
            let lifetime_proc = if !shell_args.is_empty() {
                // Spawn the SHELL on each virtual terminal (tty1..ttyN, reachable
                // via Ctrl+Alt+F1..F6) with a fixed PID 101.. — vt 0 is the
                // primary terminal and does the one-time boot work; the others
                // share its mounted root filesystem (by `Arc`, like `fork`).
                let mut shared_root = None;
                let mut primary_shell = None;
                for vt in 0..kernel_hal::console::NUM_VTS {
                    let pid = (101 + vt) as u64;
                    let proc = zcore_loader::linux::run_shell_on_vt(
                        shell_args.clone(),
                        envs.clone(),
                        rootfs.clone(),
                        vt,
                        shared_root.clone(),
                        pid,
                    );
                    if vt == 0 {
                        shared_root = Some(proc.linux().root_inode().clone());
                        primary_shell = Some(proc);
                    }
                }
                // Optionally run INIT as PID 1 (default /sbin/init -> eclipse-init,
                // busybox init fallback), if it exists. The shells already did
                // the boot work, so don't repeat.
                let init = zcore_loader::linux::run_init_if_present(
                    init_args,
                    envs.clone(),
                    rootfs.clone(),
                    shared_root,
                    false,
                );
                init.or(primary_shell)
            } else if !init_args.is_empty() {
                // No shells (e.g. libos): INIT is the single PID 1 program.
                Some(zcore_loader::linux::run(init_args, envs.clone(), rootfs.clone()))
            } else {
                None
            };
            // Make the outcome of PID 1 / base-program startup observable: log
            // which process the system's lifetime is now tied to (the PID 1
            // init when it came up, otherwise the fallback terminal shell).
            match &lifetime_proc {
                Some(p) => klog_info!(
                    "Eclipse: lifetime process pid={} name={:?}",
                    p.id(),
                    p.name()
                ),
                None => klog_info!("Eclipse: no lifetime process spawned"),
            }
            // Keep secondary CPUs idle until root is mounted and init is spawned.
            STARTED.store(true, Ordering::SeqCst);
            kernel_hal::console::early_progress_bar(100);
            utils::wait_for_exit(lifetime_proc)
        } else if #[cfg(feature = "zircon")] {
            let zbi = fs::zbi();
            kernel_hal::console::early_progress_bar(95);
            let proc = zcore_loader::zircon::run_userboot(zbi, &options.cmdline);
            STARTED.store(true, Ordering::SeqCst);
            kernel_hal::console::early_progress_bar(100);
            utils::wait_for_exit(Some(proc))
        } else {
            panic!("One of the features `linux` or `zircon` must be specified!");
        }
    }
}

/// Reads NVIDIA GSP-RM firmware from the mounted rootfs and hands it to
/// every registered DRM driver (only the real NVIDIA one acts on it --
/// see `DrmScheme::set_gsp_firmware`). Missing file is a silent no-op:
/// happens for any non-NVIDIA build, or if xtask's firmware fetch failed
/// at image-build time (best-effort, see xtask/src/linux/nvidia_firmware.rs).
#[cfg(feature = "linux")]
fn load_nvidia_gsp_firmware(root: &alloc::sync::Arc<dyn rcore_fs::vfs::INode>) {
    use alloc::string::String;
    const PATH: &str = "/lib/firmware/nvidia/gsp/gsp.bin";

    // Push a status string to every DRM driver even on failure, so a driver
    // whose set_gsp_firmware never gets called can still report *why* in its
    // /proc/gpustep6 output (the kernel log is invisible on a headless-but-
    // monitored bring-up box). all_drm() is populated during primary_init()'s
    // PCI scan, well before this runs.
    fn report(status: String) {
        for d in kernel_hal::drivers::all_drm().as_vec().iter() {
            d.set_gsp_firmware_status(status.clone());
        }
    }

    let inode = match root.lookup(PATH) {
        Ok(i) => i,
        Err(e) => {
            report(alloc::format!("lookup({PATH}) failed: {e:?}"));
            return;
        }
    };
    let size = match inode.metadata() {
        Ok(m) => m.size,
        Err(e) => {
            report(alloc::format!("metadata failed: {e:?}"));
            return;
        }
    };
    if size == 0 {
        report(String::from("file is 0 bytes"));
        return;
    }

    // Read in bounded chunks and accumulate: a single read_at over a
    // multi-megabyte buffer can fail or short-read on some filesystems (SFS
    // walks block-by-block); looping is the robust way and also handles
    // partial reads (which the old one-shot read silently truncated to the
    // first chunk).
    let mut buf: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(size);
    let mut chunk = alloc::vec![0u8; 256 * 1024];
    let mut off = 0usize;
    loop {
        match inode.read_at(off, &mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                off += n;
                if off >= size {
                    break;
                }
            }
            Err(e) => {
                // Report how far we got -- pinpoints a bad block / read cap.
                report(alloc::format!(
                    "read_at(off={off}) failed after {} of {} bytes: {e:?}",
                    buf.len(),
                    size
                ));
                if buf.is_empty() {
                    return;
                }
                break;
            }
        }
    }

    if buf.is_empty() {
        report(String::from("read produced 0 bytes"));
        return;
    }

    let n = buf.len();
    klog_info!("Eclipse: loaded NVIDIA GSP-RM firmware ({} bytes)", n);
    let drms = kernel_hal::drivers::all_drm();
    let drm_count = drms.as_vec().len();
    for d in drms.as_vec().iter() {
        d.set_gsp_firmware(buf.clone());
    }
    report(alloc::format!(
        "OK: {n} of {size} bytes delivered to {drm_count} DRM driver(s)"
    ));
}

/// Boot-time auto bring-up of every COMPUTE GPU. Iterates all registered DRM
/// drivers and asks each to bring itself up if it does NOT drive the boot
/// display (`DrmScheme::auto_bringup_compute`, a no-op for the console GPU and
/// for non-NVIDIA drivers). Best-effort: each GPU logs its own outcome and a
/// failure never aborts boot. Called once, synchronously, right after the GSP
/// firmware load and before any userspace or scanout touches RM, so the
/// copy-engine present path (P2P from a compute GPU into the console's scanout
/// FB) is ready by the time the compositor runs. General over 1, 2, 3+ GPUs:
/// a single-console-GPU box brings up nothing and falls back to the CPU blit.
#[cfg(feature = "linux")]
fn auto_bringup_compute_gpus() {
    let mut summaries = alloc::vec::Vec::new();
    // Suppress the driver's own (loud, ERROR-level) bring-up narration for the
    // whole ladder; `auto_bringup_compute` returns just one clean status line
    // per compute GPU (empty for the console GPU / non-NVIDIA drivers). The
    // detail is still captured into /proc/gpustep* for debugging.
    logging::with_output_suppressed(|| {
        for d in kernel_hal::drivers::all_drm().as_vec().iter() {
            let line = d.auto_bringup_compute();
            if !line.is_empty() {
                summaries.push(line);
            }
        }
    });
    // Emit only the tidy summary (klog_emit bypasses the level filter).
    if summaries.is_empty() {
        klog_info!("Eclipse: NVIDIA — sin GPU de cómputo; present por CPU");
    } else {
        for line in summaries {
            klog_info!("Eclipse: NVIDIA {}", line);
        }
    }
}

#[cfg(not(feature = "libos"))]
fn secondary_main() -> ! {
    // Bring up this AP (trapframe/GS/LAPIC) before STARTED so the BSP can wait
    // for AP_ONLINE during SMP init. cpu_id() uses the trampoline logical id
    // until GS is configured (APIC id from CPUID is wrong on many APs).
    kernel_hal::secondary_init();
    while !STARTED.load(Ordering::SeqCst) {
        core::hint::spin_loop();
    }
    klog_info!("Eclipse: CPU {} online", kernel_hal::cpu::cpu_id());
    #[cfg(feature = "mock-disk")]
    {
        if MOCK_CORE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            utils::mock_disk();
        }
    }
    utils::wait_for_exit(None)
}
