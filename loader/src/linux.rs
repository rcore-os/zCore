//! Run Linux process and manage trap/interrupt/syscall.

use alloc::{boxed::Box, string::String, sync::Arc, vec::Vec};
use core::{future::Future, pin::Pin};
use linux_object::signal::{
    MachineContext, SigInfo, Signal, SignalUserContext, Sigset, SIG_DFL, SIG_IGN,
};

use kernel_hal::context::{TrapReason, UserContext, UserContextField};
use kernel_hal::interrupt::intr_on;
use linux_object::fs::{
    vfs::{FileSystem, INode},
    INodeExt,
};
use linux_object::thread::{CurrentThreadExt, ThreadExt};
// `Abi` is only consumed by the x86_64-gated FreeBSD-personality paths; the
// unconditional import broke riscv64/aarch64 builds (`deny(warnings)`).
#[cfg(target_arch = "x86_64")]
use linux_object::process::Abi;
use linux_object::{loader::LinuxElfLoader, process::ProcessExt};
use zircon_object::task::{CurrentThread, Process, Thread, ThreadState};
use zircon_object::{
    object::{KernelObject, KoID},
    vm::USER_STACK_PAGES,
    ZxError, ZxResult,
};

fn comm_from_path(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Create and run a single Linux process as PID 1 on virtual terminal 0.
///
/// Used by the libos example/tests, where one program is the whole system.
pub fn run(args: Vec<String>, envs: Vec<String>, rootfs: Arc<dyn FileSystem>) -> Arc<Process> {
    const INIT_PID: KoID = 1;
    spawn(args, envs, rootfs, 0, None, INIT_PID, true, true).expect("run: program not found")
}

/// Create and run the configured per-terminal SHELL on virtual terminal `vt`
/// with the fixed Linux `pid` (the reserved 101.. range). The shell binary is
/// the system default and must exist; a missing shell is fatal.
///
/// `shared_root` lets extra per-VT shells reuse the primary shell's mounted
/// root filesystem (by `Arc`) instead of re-scanning disks. The one-time boot
/// work (network init, fstab mount, console clear) runs only for `vt == 0`.
pub fn run_shell_on_vt(
    args: Vec<String>,
    envs: Vec<String>,
    rootfs: Arc<dyn FileSystem>,
    vt: usize,
    shared_root: Option<Arc<dyn INode>>,
    pid: KoID,
) -> Arc<Process> {
    spawn(
        args,
        envs,
        rootfs,
        vt,
        shared_root,
        pid,
        /* boot_work */ vt == 0,
        /* foreground */ true,
    )
    .expect("configured SHELL not found")
}

/// Spawn the INIT process as PID 1 if its binary exists, returning `None`
/// otherwise so the machine can boot without a PID 1 init (e.g. when
/// `/bin/busybox` is not installed). Runs on the primary console (vt 0).
///
/// `boot_work` should be set only when no shell has already performed the
/// one-time boot setup (network init, fstab mount).
pub fn run_init_if_present(
    args: Vec<String>,
    envs: Vec<String>,
    rootfs: Arc<dyn FileSystem>,
    shared_root: Option<Arc<dyn INode>>,
    boot_work: bool,
) -> Option<Arc<Process>> {
    if args.is_empty() || args[0].is_empty() {
        return None;
    }
    if !program_exists(&args[0], &shared_root, &rootfs) {
        warn!(
            "INIT {:?} not present on root or initramfs; booting WITHOUT a PID 1 init (only the per-terminal shells run). Expected /sbin/init (default INIT=/sbin/init -> eclipse-init, busybox init fallback); rebuild the rootfs or set INIT= in rboot.conf.",
            args[0]
        );
        return None;
    }
    const INIT_PID: KoID = 1;
    let init = args[0].clone();
    // `foreground = false`: init shares vt 0 with the primary shell and must not
    // seize its foreground process group (that would wedge the shell on SIGTTIN).
    let proc = spawn(
        args,
        envs,
        rootfs,
        0,
        shared_root,
        INIT_PID,
        boot_work,
        false,
    );
    if proc.is_none() {
        // The binary exists but could not be started (unreadable, malformed
        // ELF, or blocked by the hunter exec policy — see the preceding
        // `spawn:`/`hunter:` console line for which). Don't silently end up
        // with no PID 1: make the fallback to the terminal shell explicit.
        warn!(
            "INIT {:?} present but FAILED to start (see the spawn/hunter line above); falling back to the shell as the lifetime process",
            init
        );
    } else {
        info!("INIT {:?} started as PID {}", init, INIT_PID);
    }
    proc
}

/// True if `path` resolves on the primary root (if any) or the initramfs.
/// Symlinks are followed (e.g. `/sbin/init` -> `/bin/busybox`), matching how
/// `spawn` loads the binary.
fn program_exists(
    path: &str,
    primary_root: &Option<Arc<dyn INode>>,
    rootfs: &Arc<dyn FileSystem>,
) -> bool {
    if let Some(root) = primary_root {
        if root.lookup_follow(path, FOLLOW_LINK_DEPTH).is_ok() {
            return true;
        }
    }
    rootfs
        .root_inode()
        .lookup_follow(path, FOLLOW_LINK_DEPTH)
        .is_ok()
}

/// Max symlink hops to follow when resolving an exec target (e.g.
/// `/sbin/init` -> `/bin/busybox`). A small bound prevents loops.
const FOLLOW_LINK_DEPTH: usize = 8;

/// Core process spawn. Returns `None` if `args[0]` cannot be found on the
/// process's root or the initramfs (instead of panicking) so optional
/// processes can be skipped gracefully. `boot_work` runs the one-time boot
/// setup (network init, fstab mount, console clear) and must happen once.
///
/// `foreground` seeds the VT's foreground process group with this process so an
/// interactive shell does not conclude it is backgrounded. It must be `false`
/// for a non-interactive PID 1 init that shares vt 0 with the primary shell —
/// otherwise init would steal vt 0's foreground group and wedge that shell on
/// `SIGTTIN`.
// Process bring-up genuinely needs all of these inputs (identity, filesystem,
// controlling terminal, job-control role); grouping them into a struct would
// only move the argument list, not reduce it.
#[allow(clippy::too_many_arguments)]
fn spawn(
    args: Vec<String>,
    envs: Vec<String>,
    rootfs: Arc<dyn FileSystem>,
    vt: usize,
    shared_root: Option<Arc<dyn INode>>,
    pid: KoID,
    boot_work: bool,
    foreground: bool,
) -> Option<Arc<Process>> {
    // Tell the process which VT it runs on. /etc/profile uses this to run the
    // serial terminal-size probe only where a reply can arrive (the serial
    // mirror follows the ACTIVE VT, i.e. VT 0 at boot) — on VTs 1..N the probe
    // could never be answered and blocked each shell ~0.3 s at boot.
    let mut envs = envs;
    envs.push(alloc::format!("ECLIPSE_VT={}", vt));
    info!(
        "spawn pid={} vt={}: args={:?}, envs={:?}",
        pid, vt, args, envs
    );
    if boot_work {
        linux_object::net::init();
        hunter::init();
    }
    let job = zircon_object::task::ROOT_JOB.clone();
    let proc =
        Process::create_linux(&job, rootfs.clone(), vt, shared_root, pid).expect("create_linux");
    let thread = Thread::create_linux(&proc).expect("create_linux thread");
    // Use the pivoted root (e.g. installed btrfs/ext2), not the initramfs SFS passed in.
    let root_inode = proc.linux().root_inode().clone();
    let loader = LinuxElfLoader {
        syscall_entry: kernel_hal::context::syscall_entry as *const () as usize,
        stack_pages: USER_STACK_PAGES,
        root_inode: root_inode.clone(),
    };

    // Follow symlinks so a symlinked entry point (e.g. /sbin/init ->
    // /bin/busybox) loads the real ELF instead of the symlink's path text.
    let inode = match root_inode.lookup_follow(&args[0], FOLLOW_LINK_DEPTH) {
        Ok(inode) => inode,
        Err(e) => match rootfs
            .root_inode()
            .lookup_follow(&args[0], FOLLOW_LINK_DEPTH)
        {
            Ok(inode) => inode,
            Err(e2) => {
                warn!(
                    "process {:?} not found on root ({:?}) or initramfs ({:?}); skipping",
                    args[0], e, e2
                );
                return None;
            }
        },
    };
    let vmo = inode
        .read_as_vmo_cached()
        .unwrap_or_else(|e| panic!("failed to read process {:?}: {:?}", args[0], e));
    let path = args[0].clone();

    // hunter P8: verify binary integrity + path policy using a full 64-byte
    // header (e_ident/e_type/e_machine), matching the runtime execve gate
    // rather than the old 4-byte magic-only check.
    let mut header = [0u8; 64];
    let _ = vmo.read(0, &mut header);
    if !hunter::check_elf_binary(&path, &header) {
        warn!("spawn: binary {:?} blocked by hunter security policy", path);
        return None;
    }

    // Boot UX: clear to black right before the first graphic-console output
    // (prompt). No-op when graphic mode is disabled. Only when this process
    // performs the one-time boot setup (the primary terminal).
    if boot_work {
        kernel_hal::console::request_clear_graphic_on_next_write();
    }

    let pg_token = kernel_hal::vm::current_vmtoken();
    debug!("current pgt = {:#x}", pg_token);
    //调用zircon-object/src/task/thread.start设置好要执行的thread
    // Likewise, a malformed/incompatible ELF for the base program must fall
    // back rather than panic the kernel.
    let (entry, sp, initial_brk, execute_path, abi) =
        match loader.load(&proc.vmar(), &vmo, args.clone(), envs, path) {
            Ok(loaded) => loaded,
            Err(e) => {
                warn!("spawn: failed to load {:?}: {:?}; skipping", args[0], e);
                return None;
            }
        };
    // Record the ABI personality the loader detected so the trap handler routes
    // this process's syscalls (and the FreeBSD carry-flag return convention)
    // correctly.
    proc.linux().set_abi(abi);
    proc.linux().set_execute_path(&execute_path);
    proc.linux().set_cmdline(args);
    proc.linux().set_brk(initial_brk);
    proc.set_name(comm_from_path(&execute_path));

    // Make this process the foreground process group of its own tty before it
    // ever runs in user mode. `getpgid` reports each process's pgrp as its own
    // pid, so seeding the VT's foreground pgrp to `pid` makes
    // `tcgetpgrp == getpgrp`; otherwise it starts at 0, the shell concludes it
    // is a background job and spins on `kill(0, SIGTTIN)` (a tight, CPU-burning
    // enter_uspace loop that was previously hidden behind the handle_signal
    // self-deadlock). Seed it only here — after every fallible step has
    // succeeded — so a process that fails to load never leaves the VT pointing
    // at a pid that never started. Skipped for a non-interactive init that
    // shares vt 0 with the primary shell (see `foreground` above).
    if foreground {
        linux_object::fs::stdio::set_vt_foreground_pgrp(vt, pid as i32);
    }

    // Entry register convention differs by ABI. FreeBSD/amd64 passes a pointer
    // to argc in %rdi and starts on an 8-mod-16 stack (`exec_setregs`,
    // sys/amd64/amd64/exec_machdep.c); Linux simply points %rsp at argc and
    // leaves the argument registers zero. `start_with_entry(entry, stack, arg1,
    // ..)` maps `stack` -> %rsp and `arg1` -> %rdi.
    #[cfg(target_arch = "x86_64")]
    let (start_sp, arg1) = if abi == Abi::Freebsd {
        (((sp - 8) & !0xf) + 8, sp)
    } else {
        (sp, 0)
    };
    #[cfg(not(target_arch = "x86_64"))]
    let (start_sp, arg1) = {
        let _ = abi;
        (sp, 0)
    };
    thread
        .start_with_entry(entry, start_sp, arg1, 0, thread_fn)
        .expect("failed to start main thread");

    // Mount the non-root /etc/fstab entries (/boot/efi vfat, /home, …) as a
    // deferred kernel task, off the synchronous boot path: the blocking
    // block-device I/O would otherwise risk stalling boot before the shell
    // shows. Done once, by whoever performs the boot work.
    if boot_work {
        kernel_hal::thread::spawn(async {
            linux_object::fs::mount_fstab_deferred();
        });
        // Periodic load-average sampler (Linux samples from the scheduler
        // tick): keeps /proc/loadavg honest by sampling the run queue at
        // instants uncorrelated with whoever reads it.
        kernel_hal::thread::spawn(linux_object::loadavg::sampler_task());
    }

    Some(proc)
}

fn thread_fn(thread: CurrentThread) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
    Box::pin(run_user(thread))
}

/// The function of a new thread.
///
/// loop:
/// - wait for the thread to be ready
/// - get user thread context
/// - enter user mode
/// - handle trap/interrupt/syscall according to the return value
/// - return the context to the user thread
async fn run_user(thread: CurrentThread) {
    kernel_hal::thread::set_current_thread(Some(thread.inner()));
    loop {
        // wait
        let mut ctx = thread.wait_for_run().await;
        if thread.state() == ThreadState::Dying {
            break;
        }

        // check the signal and handle
        //
        // Bind the result to a local FIRST so the `lock_linux()` temporary guard
        // is dropped at the end of this statement. Inlining it into the `if let`
        // scrutinee keeps the guard alive for the whole `if let` body (Rust
        // temporary scoping), and `handle_signal` re-locks the same per-thread
        // `LinuxThread` mutex — a self-deadlock on the non-reentrant TicketMutex.
        // It fires whenever a thread has a pending signal (e.g. the job-control
        // SIGTTIN a shell sends itself), wedging that core in an interrupts-off
        // spin forever: the silent multi-core busy/heat (and a hang risk).
        let pending_signal = thread.inner().lock_linux().handle_signal();
        if let Some((signal, sigmask)) = pending_signal {
            ctx = handle_signal(&thread, ctx, signal, sigmask);
        }
        if thread.state() == ThreadState::Dying {
            break;
        }

        // run
        trace!(
            "go to user: tid = {} pc = {:x} sp = {:x}",
            thread.id(),
            ctx.get_field(UserContextField::InstrPointer),
            ctx.get_field(UserContextField::StackPointer)
        );
        trace!("ctx before enter: {:#x?}", ctx);
        // Time the user-mode slice and attribute it to the thread: this is what
        // getrusage(2)/times(2) report as utime (kernel-side time is accounted
        // separately, per syscall, by linux_object::perf). checked_sub because a
        // cross-CPU migration over the entry can see unsynchronised TSCs.
        let uspace_start = kernel_hal::timer::timer_now();
        ctx.enter_uspace();
        let user_ns = kernel_hal::timer::timer_now()
            .checked_sub(uspace_start)
            .unwrap_or_default()
            .as_nanos();
        thread.time_add(user_ns);
        debug!(
            "back from user: tid = {} pc = {:x} trap reason = {:?}",
            thread.id(),
            ctx.get_field(UserContextField::InstrPointer),
            ctx.trap_reason(),
        );
        trace!("ctx = {:#x?}", ctx);
        // handle trap/interrupt/syscall
        if let Err(err) = handle_user_trap(&thread, ctx).await {
            thread.exit_linux(err as i32);
        }
        if thread.state() == ThreadState::Dying {
            break;
        }
    }
    kernel_hal::thread::set_current_thread(None);
}

fn handle_signal(
    thread: &CurrentThread,
    mut ctx: Box<UserContext>,
    signal: Signal,
    sigmask: Sigset,
) -> Box<UserContext> {
    let user_sp = ctx.get_field(UserContextField::StackPointer);
    let user_pc = ctx.get_field(UserContextField::InstrPointer);
    let action = thread.proc().linux().signal_action(signal);
    // Handle default/ignore actions without entering a handler.
    if action.handler == SIG_IGN {
        thread.inner().lock_linux().handling_signal = None;
        return ctx;
    }
    if action.handler == SIG_DFL {
        // Per-signal default disposition. Linux's default for the job-control and
        // a few status signals is NOT to terminate: SIGCHLD/SIGURG/SIGWINCH are
        // ignored, and SIGTSTP/SIGTTIN/SIGTTOU/SIGSTOP stop the process while
        // SIGCONT resumes it. This kernel has no job-control stop state, so it
        // approximates all of those as "ignore" — crucially this stops an
        // interactive `sh` from being *killed* by the SIGTTIN it sends itself
        // during job-control setup (the cause of the per-VT shells dying and the
        // terminal never reaching a usable prompt). Everything else still
        // terminates, as before.
        match signal {
            Signal::SIGCHLD
            | Signal::SIGURG
            | Signal::SIGWINCH
            | Signal::SIGCONT
            | Signal::SIGSTOP
            | Signal::SIGTSTP
            | Signal::SIGTTIN
            | Signal::SIGTTOU => {
                trace!(
                    "default-ignore signal {:?} for pid={}",
                    signal,
                    thread.proc().id()
                );
                thread.inner().lock_linux().handling_signal = None;
                return ctx;
            }
            _ => {}
        }
        let code = 128 + signal as i32;
        // Resolve addresses back to "<file>+<offset>" through the process's own
        // mappings. A bare `pc=0x499f8c` is unusable — the same number means a
        // different function in every process — while `libglib-2.0.so.0+0x1f8c`
        // names the guilty binary and can be fed straight to `addr2line` or
        // `objdump -d` on the host's copy of that file. File-backed VMOs carry
        // their path as the kernel-object name (see `File::get_vmo`).
        let maps = thread.proc().vmar().mappings_dump();
        let resolve = |addr: usize| -> Option<String> {
            let m = maps.iter().find(|m| addr >= m.start && addr < m.end)?;
            let off = m.vmo_offset + (addr - m.start);
            Some(if m.name.is_empty() {
                alloc::format!("<anon:{}>+{:#x}", m.vmo_id, off)
            } else {
                alloc::format!("{}+{:#x}", m.name, off)
            })
        };
        // Record the death in the dmesg ring at error! (survives LOG=error): a
        // process that dies on a default-disposition signal — apk killed by
        // SIGPIPE when a fetch connection resets, Xorg aborting in early init —
        // otherwise vanishes with no trace at all, and a `Done(139)`/silent exit
        // gives no clue which signal took it down.
        error!(
            "[exit] pid={} ({}) killed by signal {:?} ({}) at pc={:#x} [{}] (default disposition)",
            thread.proc().id(),
            thread.proc().name(),
            signal,
            signal as i32,
            user_pc,
            resolve(user_pc).unwrap_or_else(|| String::from("unmapped")),
        );
        // For a crash/abort signal, dump the top of the user stack: any word
        // that lands in the process's own code is a return address, so the
        // abort()/assert() CALLER chain can be read off by mapping these back
        // against the binary — turning a bare 'killed by SIGABRT' into "which
        // function aborted" without a debugger. Read through the process page
        // table's physmap image, exactly like the page-fault code-bytes dump.
        #[cfg(not(feature = "libos"))]
        if matches!(
            signal,
            Signal::SIGABRT
                | Signal::SIGSEGV
                | Signal::SIGILL
                | Signal::SIGBUS
                | Signal::SIGFPE
                | Signal::SIGTRAP
        ) {
            use kernel_hal::vm::{GenericPageTable, PageTable};
            let rsp = ctx.get_field(UserContextField::StackPointer);
            let pt = PageTable::from_current();
            // 8-aligned u64s never cross a page boundary (4096 % 8 == 0), so a
            // single physmap read per word is safe.
            let rd = |va: usize| -> Option<u64> {
                pt.query(va & !0xfff).ok().map(|(pa, _, _)| {
                    let kv = 0xffff_8000_0000_0000usize + (pa & !0xfff) + (va & 0xfff);
                    unsafe { core::ptr::read_volatile(kv as *const u64) }
                })
            };
            let base = rsp & !0x7;
            let mut words = alloc::vec::Vec::new();
            for i in 0..96usize {
                match rd(base + i * 8) {
                    Some(w) => words.push(w),
                    None => break,
                }
            }
            error!(
                "[crash] pid={} SIG={:?} rsp={:#x} stack[0..{}]={:#x?}",
                thread.proc().id(),
                signal,
                rsp,
                words.len(),
                words
            );
            // Every stack word that points into an EXECUTABLE mapping is a
            // return address, so this is the abort()/assert() caller chain,
            // in order, already resolved to file+offset. Duplicates are kept:
            // repeats are how a recursive/looping frame shows itself.
            let mut frames = alloc::vec::Vec::new();
            for w in words.iter() {
                let a = *w as usize;
                if let Some(m) = maps.iter().find(|m| a >= m.start && a < m.end) {
                    if m.flags.contains(zircon_object::vm::MMUFlags::EXECUTE) {
                        frames.push(
                            resolve(a).unwrap_or_else(|| alloc::format!("{:#x}", a)),
                        );
                    }
                }
            }
            error!(
                "[crash-bt] pid={} {} candidate return addresses (innermost first): {:?}",
                thread.proc().id(),
                frames.len(),
                frames
            );
        }
        thread.proc().exit(code as i64);
        return ctx;
    }
    let signal_info = SigInfo::default();
    let signal_context = SignalUserContext {
        sig_mask: sigmask,
        context: MachineContext::new(user_pc),
        ..Default::default()
    };
    // push `siginfo` `uctx` into user stack
    const RED_ZONE_MAX_SIZE: usize = 0x100; // 256Bytes
    let mut sp = user_sp - RED_ZONE_MAX_SIZE;
    // Always use the 3-argument SA_SIGINFO calling convention; extra args are harmless
    // for 1-argument handlers on SysV ABIs, and avoids crashing when flags are unset.
    sp = push_stack(sp & !0xF, signal_info); // & !0xF for 16 bytes aligned
    let siginfo_ptr = sp;
    sp = push_stack(sp & !0xF, signal_context);
    let uctx_ptr = sp;
    // backup current context
    thread.backup_context(*ctx, siginfo_ptr, uctx_ptr);
    // set user return address as `action.restorer`
    cfg_if! {
        if #[cfg(target_arch = "x86_64")] {
            sp = push_stack::<usize>(sp & !0xF, action.restorer);
        } else {
            ctx.set_ra(action.restorer);
        }
    }
    // set trapframe
    ctx.setup_uspace(
        action.handler,
        sp,
        &[signal as usize, siginfo_ptr, uctx_ptr],
    );
    ctx
}

/// Deliver a *synchronous* fault signal (SIGSEGV / SIGBUS / SIGILL / SIGFPE).
///
/// A faulting instruction is re-executed when the thread returns to user mode.
/// If the fault signal is merely queued but cannot actually be delivered — it is
/// blocked by the signal mask, already pending (we are re-faulting on the same
/// instruction), or a handler is already mid-flight — the thread loops on the
/// same fault forever and starves every CPU (observed as a 0x9b SIGSEGV storm,
/// ~71k faults, that hung the whole system under network load and stalled
/// downloads like `apk update`).
///
/// Mirror Linux `force_sig`: in any of those un-deliverable cases, or when the
/// disposition is the default/ignore action, terminate the process. Only a
/// not-yet-faulted custom handler gets to run, exactly once — a fault inside it
/// sets `handling_signal`, so the next fault terminates instead of looping.
fn force_fault_signal(thread: &CurrentThread, signal: Signal) {
    let action = thread.proc().linux().signal_action(signal);
    let inner = thread.inner();
    let undeliverable = {
        let linux = inner.lock_linux();
        linux.signal_mask.contains(signal)
            || linux.handling_signal.is_some()
            || linux.signals.contains(signal)
            || action.handler == SIG_DFL
            || action.handler == SIG_IGN
    };
    if undeliverable {
        warn!(
            "[exit] pid={} killed by fault signal {:?} ({}) — undeliverable, terminating",
            thread.proc().id(),
            signal,
            signal as i32,
        );
        thread.proc().exit(128 + signal as i64);
    } else {
        // Deliverable custom handler: unblock so it cannot be deferred and queue
        // it for the next pass of the run loop.
        let mut linux = inner.lock_linux();
        linux.signal_mask.remove(signal);
        linux.signals.insert(signal);
    }
}

/// Push a object onto stack
/// # Safety
///
/// This function is handling a raw pointer to the top of the stack .
pub fn push_stack<T>(stack_top: usize, val: T) -> usize {
    unsafe {
        let stack_top = (stack_top as *mut T).sub(1);
        *stack_top = val;
        stack_top as usize
    }
}

macro_rules! run_with_irq_enable {
    ($($body:tt)*) => {
        {
            intr_on();
            let ret = { $($body)* };
            kernel_hal::interrupt::intr_off();
            ret
        }
    };
}

async fn handle_user_trap(thread: &CurrentThread, mut ctx: Box<UserContext>) -> ZxResult {
    let reason = ctx.trap_reason();
    if let TrapReason::Syscall = reason {
        let num = syscall_num(&ctx);
        let args = syscall_args(&ctx);
        ctx.advance_pc(reason);
        thread.put_context(ctx);
        let mut syscall = linux_syscall::Syscall {
            thread,
            thread_fn,
            syscall_entry: kernel_hal::context::syscall_entry as *const () as usize,
        };
        // FreeBSD-personality processes speak the FreeBSD/amd64 ABI: different
        // syscall numbers and a carry-flag return convention (result in %rax,
        // secondary in %rdx, error signalled by CF with the errno in %rax). The
        // whole path is amd64-only, matching the ABI it implements.
        #[cfg(target_arch = "x86_64")]
        if thread.proc().linux().abi() == Abi::Freebsd {
            trace!("FreeBSD syscall: {} {:x?}", num, args);
            let ret = run_with_irq_enable! {
                syscall.bsd_syscall(num, args).await
            };
            thread.with_context(|ctx| {
                let g = ctx.general_mut();
                g.rax = ret.rax;
                g.rdx = ret.rdx;
                if ret.error {
                    g.rflags |= 1; // set carry: error
                } else {
                    g.rflags &= !1; // clear carry: success
                }
            })?;
            return Ok(());
        }
        trace!("Syscall: {} {:x?}", num as u32, args);
        let ret = run_with_irq_enable! {
            syscall.syscall(num as u32, args).await as usize
        };
        trace!("Syscall ret: {} -> {:x}", num as u32, ret);
        thread.with_context(|ctx| ctx.set_field(UserContextField::ReturnValue, ret))?;
        return Ok(());
    }

    thread.put_context(ctx);

    let pid = thread.proc().id();
    match reason {
        TrapReason::Interrupt(vector) => {
            kernel_hal::interrupt::handle_irq(vector);
            #[cfg(not(feature = "libos"))]
            if vector == kernel_hal::context::TIMER_INTERRUPT_VEC {
                // perf software sampling: the timer fired while this thread was
                // in user mode, so its saved PC is the interrupted user
                // instruction. Feed it to any enabled perf event (cheap no-op
                // when nothing is profiling). This gives `perf top` a live
                // user-space profile without a hardware PMU.
                let pc = thread
                    .with_context(|ctx| ctx.get_field(UserContextField::InstrPointer))
                    .unwrap_or(0);
                if pc != 0 {
                    linux_object::perf::tick(
                        thread.proc().id() as i32,
                        thread.id() as i32,
                        kernel_hal::cpu::cpu_id() as u32,
                        pc as u64,
                    );
                }
            }
            #[cfg(not(feature = "libos"))]
            {
                // Two independent reasons to give the CPU up here.
                //
                // 1. The timeslice elapsed. The slice length comes from the
                //    thread's Linux scheduling policy / nice value (see
                //    `Thread::tick_should_preempt`), so `nice` and `SCHED_*`
                //    policies give a real, observable bias in CPU share while
                //    still cutting executor churn on CPU-bound workloads.
                //
                // 2. **Wake-up preemption.** Something became runnable on this
                //    CPU while we were holding it. Without this the woken task
                //    waited for the *whole* remaining slice (up to 20 ms), because
                //    the executor only reconsiders its run queue when the polled
                //    future returns `Pending` — and a CPU-bound user thread does
                //    that only at slice expiry. That latency is invisible to any
                //    single-threaded benchmark (nothing else is ever runnable)
                //    yet it is precisely what makes an interactive session feel
                //    slow next to Linux, which preempts on wake-up via
                //    `check_preempt_curr` + a reschedule IPI. `take_need_resched`
                //    is checked on *every* interrupt vector, not just the timer,
                //    so the reschedule IPI the waker sends turns into a yield
                //    immediately instead of at the next 4 ms tick.
                let slice_expired = vector == kernel_hal::context::TIMER_INTERRUPT_VEC
                    && thread.tick_should_preempt();
                if slice_expired || kernel_hal::thread::take_need_resched() {
                    kernel_hal::thread::yield_now().await;
                }
            }
            Ok(())
        }
        TrapReason::PageFault(vaddr, flags) => {
            trace!(
                "page fault from user mode @ {:#x}({:?}), pid={}",
                vaddr,
                flags,
                pid
            );
            let vmar = thread.proc().vmar();
            let pgf_res = vmar.handle_page_fault(vaddr, flags);
            if let Err(err) = pgf_res {
                let pc = thread
                    .with_context(|ctx| ctx.get_field(UserContextField::InstrPointer))
                    .unwrap_or(0);
                // error! (not warn!): a userspace SIGSEGV is exactly what a
                // desktop bring-up needs to see, and the default log level on
                // some builds is `error`, which would otherwise discard the
                // whole crash dump below and leave a `Done(139)` with nothing
                // in dmesg to diagnose it.
                error!(
                    "unhandled page fault @ {:#x}({:?}): {:?}, pid={} proc={} pc={:#x} -> SIGSEGV",
                    vaddr,
                    flags,
                    err,
                    pid,
                    thread.proc().name(),
                    pc,
                );
                // Name the mapping the faulting PC and the bad address live in,
                // as `<file> + 0x<file-offset>`. For a dynamically-linked crash
                // (labwc + libwlroots + …) this is what lets the exact fault be
                // symbolised offline (`addr2line -e <file> 0x<off>`), turning a
                // bare `Done(139)` into a located crash. Empty name = anonymous
                // memory (heap/stack/JIT), still useful as `[anon] + off`.
                let describe = |what: &str, va: usize| {
                    if let Some((base, file_off, name)) = vmar.describe_addr(va) {
                        let label = if name.is_empty() { "[anon]" } else { &name };
                        error!(
                            "[crash] pid={} {} {:#x} in {} + {:#x} (map base {:#x})",
                            pid, what, va, label, file_off, base,
                        );
                    }
                };
                describe("pc", pc);
                describe("fault-addr", vaddr as usize);
                // NOT_FOUND means no VmMapping covers `vaddr`. Show the
                // neighbours so the next occurrence says WHY: a region that
                // ends just short of the address (created too small, or cut
                // back) versus nothing mapped anywhere near (mmap returned a
                // range it never installed). The open bug has malloc handing
                // out a page-aligned pointer to unmapped memory, and these two
                // cases need completely different fixes.
                if err == ZxError::NOT_FOUND {
                    thread.proc().vmar().dump_near(vaddr, 0x20000);
                }
                // Make a userspace crash self-diagnosing from dmesg: dump the
                // registers and the code bytes around the faulting PC. With the
                // faulting instruction *and* the instructions that computed the
                // bad pointer, plus the register values, the root cause (botched
                // relocation/GOT, bad TLS access, AVX op, …) can be read off
                // directly instead of guessed.
                // x86_64-only: the register dump names x86 GPRs (rax/rbx/…).
                #[cfg(all(not(feature = "libos"), target_arch = "x86_64"))]
                {
                    use kernel_hal::vm::{GenericPageTable, PageTable};
                    let pt = PageTable::from_current();
                    // Read one user byte through the process page table (physmap).
                    let rd = |va: usize| -> Option<u8> {
                        pt.query(va & !0xfff).ok().map(|(pa, _, _)| {
                            let kv = 0xffff_8000_0000_0000usize + (pa & !0xfff) + (va & 0xfff);
                            unsafe { core::ptr::read_volatile(kv as *const u8) }
                        })
                    };
                    if let Some((rax, rbx, rcx, rdx, rsi, rdi, rbp, r8to11)) = thread
                        .with_context(|ctx| {
                            let g = ctx.general();
                            (g.rax, g.rbx, g.rcx, g.rdx, g.rsi, g.rdi, g.rbp, g.r8)
                        })
                        .ok()
                    {
                        error!(
                            "[crash] pid={} pc={:#x} rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} rsi={:#x} rdi={:#x} rbp={:#x} r8={:#x}",
                            pid, pc, rax, rbx, rcx, rdx, rsi, rdi, rbp, r8to11,
                        );
                        // Heap forensics. The open xfce4-session crash has the
                        // musl mallocng shape: `mov -0x10(%rdi),%rax` returns 0
                        // from a MAPPED address, so get_meta()'s
                        // `assert(meta->mem == base)` faults at 0x10. The whole
                        // investigation turns on WHAT that still-mapped page
                        // holds and WHICH frame backs it, so dump the PTE plus
                        // 64 bytes for the pointer registers and for the page
                        // base. Then:
                        //   * one zeroed qword, neighbours intact -> narrow
                        //     stray write;
                        //   * the whole page zero -> the mapping was replaced or
                        //     the frame re-zeroed (hunt the mapping layer; `pa`
                        //     names the frame);
                        //   * 0x5a5a.. (build with --features mem-debug, which
                        //     poisons freed frames) -> a stale PTE onto a FREED
                        //     frame, i.e. use-after-free of a physical frame.
                        // Also check the two bytes at rdi-2: mallocng's in-band
                        // `offset`. If only that is zero the group header may be
                        // intact and an application double-free is back in play
                        // (free() itself writes *(uint16_t*)(p-2) = 0).
                        let dump = |tag: &str, va: usize| {
                            if va == 0 {
                                return;
                            }
                            match pt.query(va & !0xfff) {
                                Ok((pa, mmu_flags, _)) => {
                                    let mut bytes = [0u8; 64];
                                    for (i, b) in bytes.iter_mut().enumerate() {
                                        if let Some(byte) = rd(va.wrapping_add(i)) {
                                            *b = byte;
                                        }
                                    }
                                    error!(
                                        "[crash] pid={} {}={:#x} pa={:#x} flags={:?} bytes={:02x?}",
                                        pid, tag, va, pa, mmu_flags, bytes,
                                    );
                                }
                                Err(_) => error!(
                                    "[crash] pid={} {}={:#x} NOT MAPPED in the page table",
                                    pid, tag, va,
                                ),
                            }
                        };
                        dump("rcx", rcx);
                        dump("rdi", rdi);
                        dump("rcx_page", rcx & !0xfff);
                        dump("rbx", rbx);
                    }
                    // 16 bytes before + 32 after the PC: shows how the faulting
                    // pointer register was loaded, and the faulting instruction.
                    let mut code = [0u8; 48];
                    let start = pc.wrapping_sub(16);
                    let mut any = false;
                    for (i, b) in code.iter_mut().enumerate() {
                        if let Some(byte) = rd(start.wrapping_add(i)) {
                            *b = byte;
                            any = true;
                        }
                    }
                    if any {
                        error!(
                            "[crash] pid={} code@{:#x} (pc-16): {:02x?}",
                            pid, start, code
                        );
                    } else {
                        error!(
                            "[crash] pid={} pc={:#x} UNMAPPED (jumped through a bad pointer)",
                            pid, pc
                        );
                    }
                }
                force_fault_signal(thread, Signal::SIGSEGV);
            }
            Ok(())
        }
        TrapReason::UndefinedInstruction => {
            warn!("undefined instruction from user mode, pid={}", pid);
            force_fault_signal(thread, Signal::SIGILL);
            Ok(())
        }
        TrapReason::SoftwareBreakpoint | TrapReason::HardwareBreakpoint => {
            warn!("breakpoint from user mode, pid={}", pid);
            thread.inner().lock_linux().signals.insert(Signal::SIGTRAP);
            Ok(())
        }
        TrapReason::UnalignedAccess => {
            warn!("unaligned access from user mode, pid={}", pid);
            force_fault_signal(thread, Signal::SIGBUS);
            Ok(())
        }
        TrapReason::GernelFault(trap_num) => {
            let signal = cpu_fault_signal(trap_num);
            let pc = thread
                .with_context(|ctx| ctx.get_field(UserContextField::InstrPointer))
                .unwrap_or(0);
            warn!(
                "cpu fault from user mode: trap={:#x} -> {:?}, pid={}, tid={}, pc={:#x}",
                trap_num,
                signal,
                pid,
                thread.id(),
                pc
            );
            force_fault_signal(thread, signal);
            Ok(())
        }
        _ => {
            error!(
                "unsupported trap from user mode: {:x?}, pid={}, {:#x?}",
                reason,
                pid,
                thread.context_cloned(),
            );
            Err(ZxError::NOT_SUPPORTED)
        }
    }
}

/// Map a CPU exception (trap) number to the appropriate Linux signal.
///
/// On x86_64 the mapping follows Linux kernel conventions from
/// `arch/x86/kernel/traps.c`.  On other architectures a conservative
/// default of SIGSEGV is used.
fn cpu_fault_signal(trap_num: usize) -> Signal {
    cfg_if! {
        if #[cfg(target_arch = "x86_64")] {
            match trap_num as u8 {
                0x00 => Signal::SIGFPE,   // #DE  Divide Error
                0x04 => Signal::SIGSEGV,  // #OF  Overflow
                0x05 => Signal::SIGSEGV,  // #BR  Bound-Range Exceeded
                0x07 => Signal::SIGFPE,   // #NM  Device Not Available (no FPU)
                0x08 => Signal::SIGKILL,  // #DF  Double Fault
                0x09 => Signal::SIGFPE,   // Coprocessor Segment Overrun
                0x0a => Signal::SIGSEGV,  // #TS  Invalid TSS
                0x0b => Signal::SIGBUS,   // #NP  Segment Not Present
                0x0c => Signal::SIGSEGV,  // #SS  Stack-Segment Fault
                0x0d => Signal::SIGSEGV,  // #GP  General Protection Fault
                0x10 => Signal::SIGFPE,   // #MF  x87 FP Exception
                0x13 => Signal::SIGFPE,   // #XF  SIMD FP Exception
                _    => Signal::SIGSEGV,
            }
        } else {
            let _ = trap_num;
            Signal::SIGSEGV
        }
    }
}

fn syscall_num(ctx: &UserContext) -> usize {
    let regs = ctx.general();
    cfg_if! {
        if #[cfg(target_arch = "x86_64")] {
            regs.rax
        } else if #[cfg(target_arch = "aarch64")] {
            regs.x8
        } else if #[cfg(target_arch = "riscv64")] {
            regs.a7
        } else {
            unimplemented!()
        }
    }
}

fn syscall_args(ctx: &UserContext) -> [usize; 6] {
    let regs = ctx.general();
    cfg_if! {
        if #[cfg(target_arch = "x86_64")] {
            [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9]
        } else if #[cfg(target_arch = "aarch64")] {
            [regs.x0, regs.x1, regs.x2, regs.x3, regs.x4, regs.x5]
        } else if #[cfg(target_arch = "riscv64")] {
            [regs.a0, regs.a1, regs.a2, regs.a3, regs.a4, regs.a5]
        } else {
            unimplemented!()
        }
    }
}
