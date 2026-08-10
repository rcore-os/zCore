use super::*;
use core::fmt::Debug;
use core::mem::size_of;

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use bitflags::bitflags;

use kernel_hal::context::{UserContext, UserContextField};
use linux_object::error::LxResult;
use linux_object::fs::{FileLike, PidFd};
use linux_object::process::{wait_child, wait_child_any};
use linux_object::signal::SigInfo;
use linux_object::thread::{CurrentThreadExt, RobustList, ThreadExt};
use linux_object::time::RUsage;
use linux_object::time::TimeSpec;
use linux_object::{fs::INodeExt, loader::LinuxElfLoader};
use zircon_object::object::{KernelObject, KoID, Signal};
use zircon_object::task::{
    Status, MAX_NICE, MAX_RT_PRIO, MIN_NICE, MIN_RT_PRIO, SCHED_BATCH, SCHED_DEADLINE, SCHED_FIFO,
    SCHED_IDLE, SCHED_NORMAL, SCHED_RR,
};
use zircon_object::vm::USER_STACK_PAGES;

const P_ALL: i32 = 0;
const P_PID: i32 = 1;
const P_PGID: i32 = 2;
// Linux <linux/wait.h>: P_PIDFD == 3 (this was wrongly 5, so waitid() with a
// pidfd — as glib's g_child_watch_source_new() uses — matched no arm and
// returned EINVAL).
const P_PIDFD: i32 = 3;

/// `SCHED_RESET_ON_FORK`: OR-ed into the policy by `sched_setscheduler` /
/// `sched_setattr`. Accepted but not modelled (we never fork-reset).
const SCHED_RESET_ON_FORK: usize = 0x4000_0000;
/// `setpriority`/`getpriority` `which`: operate on a process (by PID).
const PRIO_PROCESS: usize = 0;
/// `setpriority`/`getpriority` `which`: operate on a process group.
const PRIO_PGRP: usize = 1;
/// `setpriority`/`getpriority` `which`: operate on a user (by UID).
const PRIO_USER: usize = 2;

/// Linux `struct sched_attr` (the v0 / 48-byte layout) used by
/// `sched_setattr(2)` / `sched_getattr(2)`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SchedAttr {
    /// Size of this structure in bytes.
    pub size: u32,
    /// Scheduling policy (a `SCHED_*` constant).
    pub sched_policy: u32,
    /// Scheduling flags (e.g. `SCHED_FLAG_RESET_ON_FORK`).
    pub sched_flags: u64,
    /// Nice value for the fair policies (-20..=19).
    pub sched_nice: i32,
    /// Static priority for the real-time policies (1..=99).
    pub sched_priority: u32,
    /// `SCHED_DEADLINE` runtime in nanoseconds (unused here).
    pub sched_runtime: u64,
    /// `SCHED_DEADLINE` relative deadline in nanoseconds (unused here).
    pub sched_deadline: u64,
    /// `SCHED_DEADLINE` period in nanoseconds (unused here).
    pub sched_period: u64,
}

fn write_sigchld_info(mut infop: UserOutPtr<SigInfo>, pid: KoID, status: i32) -> SysResult {
    if infop.is_null() {
        return Ok(0);
    }
    infop.write(SigInfo::child_exited(pid as i32, status))?;
    Ok(0)
}

fn is_child_process(
    parent: &zircon_object::task::Process,
    child: &zircon_object::task::Process,
) -> bool {
    parent.linux().has_child(child.id())
}

fn comm_from_path(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Syscalls for process.
///
/// # Menu
///
/// - [`fork`](Self::sys_fork)
/// - [`vfork`](Self::sys_vfork)
/// - [`clone`](Self::sys_clone)
/// - [`wait4`](Self::sys_wait4)
/// - [`execve`](Self::sys_execve)
/// - [`gettid`](Self::sys_gettid)
/// - [`getpid`](Self::sys_getpid)
/// - [`getppid`](Self::sys_getppid)
/// - [`exit`](Self::sys_exit)
/// - [`exit_group`](Self::sys_exit_group)
/// - [`nanosleep`](Self::sys_nanosleep)
/// - [`set_tid_address`](Self::sys_set_tid_address)
impl Syscall<'_> {
    /// `fork` creates a new process by duplicating the calling process
    /// (see [linux man fork(2)](https://www.man7.org/linux/man-pages/man2/fork.2.html)).
    /// The new process is referred to as the child process.
    /// The calling process is referred to as the parent process.
    ///
    /// The child process and the parent process run in separate memory spaces.
    /// At the time of `fork` both memory spaces have the same content.
    /// Memory writes, file mappings ([`Self::sys_mmap`]) and unmappings ([`Self::sys_munmap`])
    /// performed by one of the processes do not affect the other.
    ///
    /// The child process is an exact duplicate of the parent process except for the following points:
    ///
    /// - The child has its own unique process ID, and this PID does not match the ID of any existing process.
    /// - The child's parent process ID is the same as the parent's process ID.
    /// - Process resource utilizations ([`Self::sys_getrusage`]) and CPU time counters ([`Self::sys_times`]) are reset to zero in the child.
    /// - The child does not inherit semaphore adjustments from its parent ([`Self::sys_semop`]).
    /// - The child does not inherit process-associated record locks from its parent ([`Self::sys_fcntl`]).
    ///   (On the other hand, it does inherit [`Self::sys_fcntl`] open file description locks and [`Self::sys_flock`] locks from its parent.)
    ///
    /// Note the following further points:
    ///
    /// - The child process is created with a single thread—the one that called fork().
    ///   The entire virtual address space of the parent is replicated in the child,
    ///   including the states of mutexes and condition variables.
    /// - After a `fork` in a multithreaded program,
    ///   the child can safely call only async-signal-safe functions
    ///   until such time as it calls [`Self::sys_execve`].
    /// - The child inherits copies of the parent's set of open file descriptors.
    ///   Each file descriptor in the child refers to the same open file description (see [`Self::sys_open`])
    ///   as the corresponding file descriptor in the parent.
    ///   This means that the two file descriptors share open file status flags and file offset.
    fn fork_impl(&self, newsp: usize, newtls: usize) -> LxResult<Arc<Process>> {
        info!("fork: newsp={:#x} newtls={:#x}", newsp, newtls);
        let new_proc = Process::fork_from(self.zircon_process(), false)?; // old pt NULL here
        let path = new_proc.linux().execute_path();
        if !path.is_empty() {
            new_proc.set_name(comm_from_path(&path));
        }
        let new_thread = Thread::create_linux(&new_proc)?;
        let mut new_ctx = self.thread.context_cloned()?;
        if newsp != 0 {
            new_ctx.set_field(UserContextField::StackPointer, newsp);
        }
        if newtls != 0 {
            new_ctx.set_field(UserContextField::ThreadPointer, newtls);
        }
        new_ctx.set_field(UserContextField::ReturnValue, 0);
        // A FreeBSD child returns 0 in %rax, 1 in %rdx and a clear carry flag
        // (cpu_fork, sys/amd64/amd64/vm_machdep.c). libc's fork() stub branches
        // on carry, so a stale CF inherited from the parent's context would make
        // the child believe fork() failed. Linux needs only %rax = 0.
        #[cfg(target_arch = "x86_64")]
        if self.linux_process().abi() == linux_object::process::Abi::Freebsd {
            let g = new_ctx.general_mut();
            g.rdx = 1;
            g.rflags &= !1;
        }
        new_thread.with_context(|ctx| *ctx = new_ctx)?;
        new_thread.start(self.thread_fn)?;
        // hunter: inherit the parent's syscall whitelist into the child so a
        // process cannot shed its policy merely by forking, and seed a fresh
        // anomaly window for the new pid.
        hunter::task_fork(self.zircon_process().id(), new_proc.id());
        info!("fork: {} -> {}", self.zircon_process().id(), new_proc.id());
        Ok(new_proc)
    }

    async fn vfork_impl(&self, newsp: usize, newtls: usize) -> LxResult<Arc<Process>> {
        info!("vfork: newsp={:#x} newtls={:#x}", newsp, newtls);
        self.zircon_process().vmar().dump();
        let new_proc = Process::fork_from(self.zircon_process(), true)?;
        new_proc.vmar().dump();
        let new_thread = Thread::create_linux(&new_proc)?;
        let mut new_ctx = self.thread.context_cloned()?;
        if newsp != 0 {
            new_ctx.set_field(UserContextField::StackPointer, newsp);
        }
        if newtls != 0 {
            new_ctx.set_field(UserContextField::ThreadPointer, newtls);
        }
        new_ctx.set_field(UserContextField::ReturnValue, 0);
        // A FreeBSD child returns 0 in %rax, 1 in %rdx and a clear carry flag
        // (cpu_fork, sys/amd64/amd64/vm_machdep.c). libc's fork() stub branches
        // on carry, so a stale CF inherited from the parent's context would make
        // the child believe fork() failed. Linux needs only %rax = 0.
        #[cfg(target_arch = "x86_64")]
        if self.linux_process().abi() == linux_object::process::Abi::Freebsd {
            let g = new_ctx.general_mut();
            g.rdx = 1;
            g.rflags &= !1;
        }
        new_thread.with_context(|ctx| *ctx = new_ctx)?;
        new_thread.start(self.thread_fn)?;
        // hunter: same lifecycle hook as fork (see fork_impl).
        hunter::task_fork(self.zircon_process().id(), new_proc.id());

        let new_proc_obj: Arc<dyn KernelObject> = new_proc.clone();
        info!(
            "vfork: {} -> {}. Waiting for execve SIGNALED",
            self.zircon_process().id(),
            new_proc.id()
        );
        new_proc_obj
            .wait_signal(Signal::USER_SIGNAL_0 | Signal::PROCESS_TERMINATED)
            .await; // wait for execve or termination
        Ok(new_proc)
    }

    /// `sys_fork` creates a child process.
    pub fn sys_fork(&self, newsp: usize, newtls: usize) -> SysResult {
        self.fork_impl(newsp, newtls).map(|proc| proc.id() as usize)
    }

    /// `sys_vfork` creates a child process and blocks the parent until the child terminates or execs.
    pub async fn sys_vfork(&self, newsp: usize, newtls: usize) -> SysResult {
        self.vfork_impl(newsp, newtls)
            .await
            .map(|proc| proc.id() as usize)
    }

    /// `sys_clone` create a new thread in the current process.
    /// The new thread's stack pointer will be set to `newsp`,
    /// and thread pointer will be set to `newtls`.
    /// The child TID will be stored at both `parent_tid` and `child_tid`.
    ///
    /// > **NOTE!** This system call is not exactly the same as `clone` in Linux.
    ///
    /// > **NOTE!** This is partially implemented for `musl` only.
    pub async fn sys_clone(
        &self,
        flags: usize,
        newsp: usize,
        mut parent_tid: UserOutPtr<i32>,
        newtls: usize,
        mut child_tid: UserOutPtr<i32>,
    ) -> SysResult {
        let clone_flags = CloneFlags::from_bits_truncate(flags);
        info!(
            "clone: flags={:#x}, newsp={:#x}, parent_tid={:?}, child_tid={:?}, newtls={:#x}",
            flags, newsp, parent_tid, child_tid, newtls
        );
        if clone_flags.contains(CloneFlags::PIDFD) && clone_flags.contains(CloneFlags::THREAD) {
            return Err(LxError::EINVAL);
        }
        // This kernel has no namespaces. `from_bits_truncate` silently DROPPED
        // the CLONE_NEW* bits, so a caller asking for a new mount/user/pid
        // namespace got a plain fork and was told it succeeded -- a lie with
        // consequences, since the whole point of those flags is isolation the
        // child does not actually have.
        //
        // Linux built without namespace support answers EINVAL, and that is
        // both the honest answer and the useful one: bubblewrap turns it into
        //   bwrap: Creating new namespace failed, likely because the kernel
        //          does not support user namespaces.
        // which is the string glycin matches to decide the sandbox is
        // unavailable and run its image loaders directly. It also matches the
        // ENOSYS this kernel returns from `unshare` -- the two entry points to
        // the same feature must not disagree.
        const UNSUPPORTED_NS: CloneFlags = CloneFlags::from_bits_truncate(
            CloneFlags::NEWNS.bits()
                | CloneFlags::NEWCGROUP.bits()
                | CloneFlags::NEWUTS.bits()
                | CloneFlags::NEWIPC.bits()
                | CloneFlags::NEWUSER.bits()
                | CloneFlags::NEWPID.bits()
                | CloneFlags::NEWNET.bits(),
        );
        if clone_flags.intersects(UNSUPPORTED_NS) {
            warn!(
                "clone: rejecting unsupported namespace flags {:#x} (no namespaces in this kernel)",
                flags & UNSUPPORTED_NS.bits()
            );
            return Err(LxError::EINVAL);
        }
        // Fork-like clones: if the THREAD bit is not set, the caller wants a
        // new process. This covers SIGCHLD (0x11), VFORK|VM|SIGCHLD (0x4111)
        // and other combinations used by musl/glibc fork/posix_spawn/system().
        if !clone_flags.contains(CloneFlags::THREAD) {
            // The TLS argument is meaningful ONLY with CLONE_SETTLS -- exactly
            // as the thread path below already treats it. Without the gate the
            // child's thread pointer was set from whatever happened to be in
            // the tls register, and callers legitimately leave it undefined:
            // bubblewrap's `raw_clone` is
            //     syscall (__NR_clone, flags, child_stack)
            // and musl's variadic `syscall()` always reads SIX varargs, so the
            // unpassed parent_tid/child_tid/tls come from the register save
            // area. bwrap's child got a thread pointer of 0xffffff9c -- a
            // leftover AT_FDCWD (-100) -- and died on its very first TLS access:
            //     unhandled page fault @ 0xffffff9c(READ|USER) proc=bwrap
            //     pc=0x477225   ->   mov %fs:0x0,%rbx
            // inside musl's __syscall_cp_c, i.e. before any of bwrap's own code
            // could run. parent_tid and child_tid were already gated on their
            // flags for the same reason; tls was the one that was not.
            let tls = if clone_flags.contains(CloneFlags::SETTLS) {
                newtls
            } else {
                0
            };
            let process = if clone_flags.contains(CloneFlags::VFORK) {
                info!("sys_clone: dispatching to sys_vfork for flags {:#x}", flags);
                self.vfork_impl(newsp, tls).await?
            } else {
                info!("sys_clone: dispatching to sys_fork for flags {:#x}", flags);
                self.fork_impl(newsp, tls)?
            };
            let pid = process.id() as usize;

            if clone_flags.contains(CloneFlags::PIDFD) {
                let pidfd =
                    linux_object::fs::PidFd::new(process, linux_object::fs::OpenFlags::CLOEXEC);
                let fd = self.linux_process().add_file(pidfd)?;
                parent_tid.write(fd.into())?;
            }
            return Ok(pid);
        }
        // Thread creation. Accept any CLONE_THREAD combination instead of the
        // two exact musl flag values: glibc's pthread_create passes 0x3d0f00
        // (no CLONE_DETACHED), and falling back to fork() for it silently
        // created a separate process whose "threads" could never synchronize
        // through futexes with the parent.
        let new_thread = Thread::create_linux(self.zircon_process())?;
        let mut new_ctx = self.thread.context_cloned()?;
        new_ctx.set_field(UserContextField::StackPointer, newsp);
        if clone_flags.contains(CloneFlags::SETTLS) {
            new_ctx.set_field(UserContextField::ThreadPointer, newtls);
        }
        new_ctx.set_field(UserContextField::ReturnValue, 0);
        // A FreeBSD child returns 0 in %rax, 1 in %rdx and a clear carry flag
        // (cpu_fork, sys/amd64/amd64/vm_machdep.c). libc's fork() stub branches
        // on carry, so a stale CF inherited from the parent's context would make
        // the child believe fork() failed. Linux needs only %rax = 0.
        #[cfg(target_arch = "x86_64")]
        if self.linux_process().abi() == linux_object::process::Abi::Freebsd {
            let g = new_ctx.general_mut();
            g.rdx = 1;
            g.rflags &= !1;
        }
        new_thread.with_context(|ctx| *ctx = new_ctx)?;

        let tid = new_thread.id();
        info!("clone: {} -> {}", self.thread.id(), tid);
        // Honor the TID bookkeeping flags BEFORE the thread starts running:
        // the child and the parent's pthread library may read these
        // immediately. In particular, ctid must only be written here when
        // CLONE_CHILD_SETTID is set — musl points ctid at its global
        // __thread_list_lock (for the CLONE_CHILD_CLEARTID exit wake), and
        // unconditionally storing the TID there corrupts that lock.
        if clone_flags.contains(CloneFlags::PARENT_SETTID) {
            parent_tid.write_if_not_null(tid as i32)?;
        }
        if clone_flags.contains(CloneFlags::CHILD_SETTID) {
            child_tid.write_if_not_null(tid as i32)?;
        }
        if clone_flags.contains(CloneFlags::CHILD_CLEARTID) {
            new_thread.set_tid_address(child_tid);
        }
        new_thread.start(self.thread_fn)?;
        Ok(tid as usize)
    }

    /// `clone3` — the extensible successor of `clone`. glibc ≥ 2.34 tries it
    /// FIRST for `fork()`, `pthread_create()` and `posix_spawn()` and only
    /// falls back to legacy `clone` on ENOSYS, so answering it natively keeps
    /// glibc userspace on its primary path (and the log free of
    /// `unknown syscall: CLONE3`).
    ///
    /// Reads `struct clone_args` from `uargs` (`size` bytes, ≥ 64):
    /// flags / pidfd / child_tid / parent_tid / exit_signal / stack /
    /// stack_size / tls, all u64. Differences from legacy `clone` handled
    /// here:
    ///  * flags and the exit signal are separate fields (legacy packs the
    ///    signal into the low byte of flags);
    ///  * `stack` is the LOW address of the child stack and the kernel
    ///    computes the initial SP as `stack + stack_size` (legacy passes the
    ///    top directly);
    ///  * the pidfd (CLONE_PIDFD) has its own output pointer instead of
    ///    sharing `parent_tid`.
    ///
    /// Everything else is delegated to [`sys_clone`](Self::sys_clone).
    pub async fn sys_clone3(&self, uargs: UserInPtr<u64>, size: usize) -> SysResult {
        const CLONE_ARGS_SIZE_VER0: usize = 64;
        if size < CLONE_ARGS_SIZE_VER0 {
            return Err(LxError::EINVAL);
        }
        let words = uargs.read_array(CLONE_ARGS_SIZE_VER0 / 8)?;
        let [flags, pidfd, child_tid, parent_tid, exit_signal, stack, stack_size, tls] =
            <[u64; 8]>::try_from(words).unwrap();
        info!(
            "clone3: flags={:#x} exit_signal={} stack={:#x} stack_size={:#x}",
            flags, exit_signal, stack, stack_size
        );
        // The exit signal lives in its own field; flags carrying low-byte bits
        // is invalid here, as is an out-of-range signal number.
        if flags & 0xff != 0 || exit_signal > 64 {
            return Err(LxError::EINVAL);
        }
        if (stack == 0) != (stack_size == 0) {
            return Err(LxError::EINVAL);
        }
        let newsp = if stack != 0 {
            (stack + stack_size) as usize
        } else {
            0
        };
        let clone_flags = CloneFlags::from_bits_truncate(flags as usize);
        // Legacy clone reports the pidfd through the parent_tid slot; clone3
        // gives it a dedicated field. PARENT_SETTID and PIDFD together can't
        // be expressed through the legacy entry point — glibc never combines
        // them, so reject rather than misdeliver one of the two.
        let parent_slot = if clone_flags.contains(CloneFlags::PIDFD) {
            if clone_flags.contains(CloneFlags::PARENT_SETTID) {
                return Err(LxError::EINVAL);
            }
            pidfd
        } else {
            parent_tid
        };
        self.sys_clone(
            flags as usize | exit_signal as usize,
            newsp,
            (parent_slot as usize).into(),
            tls as usize,
            (child_tid as usize).into(),
        )
        .await
    }

    /// `sys_wait4` suspends execution of the calling thread
    /// until a child specified by `pid` argument has changed state
    /// (see [linux man wait4(2)](https://www.man7.org/linux/man-pages/man2/wait4.2.html)).
    /// By default, `sys_wait4` waits only for terminated children,
    /// but this behavior is modifiable via the options argument, as described below.
    ///
    /// The value of `pid` can be:
    ///
    /// - **-1**: meaning wait for any child process.
    /// - **0**: meaning wait for any child process whose process group ID is equal to
    ///   that of the calling process at the time of the call to `sys_wait4`.
    /// - **>0**: meaning wait for the child whose process ID is equal to the value of `pid`.
    ///
    /// The value of options is an OR of zero or more of the following constants:
    ///
    /// - **NOHANG**    = 0x000_0001;
    ///
    ///   TODO
    ///
    /// - **STOPPED**   = 0x000_0002;
    ///
    ///   TODO
    ///
    /// - **EXITED**    = 0x000_0004;
    ///
    ///   TODO
    ///
    /// - **CONTINUED** = 0x000_0008;
    ///
    ///   TODO
    ///
    /// - **NOWAIT**    = 0x100_0000;
    ///
    ///   TODO
    ///
    /// On success, returns the process ID of the child whose state has changed;
    /// if `NOHANG` flag was specified and one or more child(ren) specified by pid exist,
    /// but have not yet changed state, then 0 is returned.
    /// On failure, -1 is returned.
    pub async fn sys_wait4(
        &self,
        pid: i32,
        mut wstatus: UserOutPtr<i32>,
        options: u32,
        mut rusage: UserOutPtr<RUsage>,
    ) -> SysResult {
        #[derive(Debug)]
        enum WaitTarget {
            AnyChild,
            AnyChildInGroup,
            Pid(KoID),
        }
        bitflags! {
            struct WaitFlags: u32 {
                const NOHANG    = 1;
                const STOPPED   = 2;
                const EXITED    = 4;
                const CONTINUED = 8;
                const NOWAIT    = 0x100_0000;
            }
        }
        let target = match pid {
            -1 => WaitTarget::AnyChild,
            0 => WaitTarget::AnyChildInGroup,
            p if p > 0 => WaitTarget::Pid(p as KoID),
            // pid < -1 means "any child in process group |pid|". Process groups
            // are not tracked here, so fall back to waiting on any child rather
            // than panicking the kernel on user-controlled input.
            _ => WaitTarget::AnyChildInGroup,
        };
        let flags = WaitFlags::from_bits_truncate(options);
        let nohang = flags.contains(WaitFlags::NOHANG);
        // Consume (reap) the child's exit status unless WNOWAIT was requested,
        // which only peeks at it and leaves the zombie for a later wait.
        let reap = !flags.contains(WaitFlags::NOWAIT);
        // Hot path (shells, fork+exec, sysbench worker reaping): keep at debug
        // so a default `LOG=warn` boot doesn't pay a synchronous serial write
        // on every wait.
        debug!(
            "wait4: target={:?}, wstatus={:?}, options={:?}",
            target, wstatus, flags,
        );
        let result = match target {
            WaitTarget::AnyChild | WaitTarget::AnyChildInGroup => {
                wait_child_any(self.zircon_process(), nohang, reap).await
            }
            WaitTarget::Pid(pid) => wait_child(self.zircon_process(), pid, nohang, reap)
                .await
                .map(|(code, cpu)| (pid, code, cpu)),
        };
        let (pid, code, cpu) = match result {
            Ok(tuple) => tuple,
            Err(LxError::EAGAIN) if nohang => {
                // WNOHANG: no child ready yet — return 0 per POSIX waitpid(2).
                wstatus.write_if_not_null(0)?;
                return Ok(0);
            }
            Err(e) => return Err(e),
        };
        wstatus.write_if_not_null(code)?;
        // The child's final CPU usage, captured at its exit — what `time(1)`
        // prints. The argument used to be dropped entirely, leaving callers to
        // read whatever stack garbage sat in their buffer; the struct is
        // written in the FULL Linux layout.
        rusage.write_if_not_null(RUsage {
            utime: core::time::Duration::from_nanos(cpu.utime_ns).into(),
            stime: core::time::Duration::from_nanos(cpu.stime_ns).into(),
            ..RUsage::default()
        })?;
        Ok(pid as usize)
    }

    /// Wait for a child state change (`waitid(2)`). Supports `P_PID`, `P_PIDFD`, and `P_ALL`.
    pub async fn sys_waitid(
        &self,
        idtype: i32,
        id: usize,
        infop: UserOutPtr<SigInfo>,
        options: u32,
    ) -> SysResult {
        // Valid options mask: WNOHANG | WSTOPPED | WEXITED | WCONTINUED | WNOWAIT | __WNOTHREAD | __WCLONE | __WALL
        let valid_mask = 0x0100_0000
            | 0x0000_0001
            | 0x0000_0002
            | 0x0000_0004
            | 0x0000_0008
            | 0x2000_0000
            | 0x4000_0000
            | 0x8000_0000;
        if (options & !valid_mask) != 0 {
            return Err(LxError::EINVAL);
        }
        // At least one of WEXITED, WSTOPPED, WCONTINUED must be specified
        let required_mask = 0x0000_0002 | 0x0000_0004 | 0x0000_0008;
        if (options & required_mask) == 0 {
            return Err(LxError::EINVAL);
        }

        bitflags! {
            struct WaitIdOptions: u32 {
                const WNOHANG   = 0x0000_0001;
                const WSTOPPED  = 0x0000_0002;
                const WEXITED   = 0x0000_0004;
                const WCONTINUED = 0x0000_0008;
                const WNOWAIT   = 0x0100_0000;
                const WNOTHREAD = 0x2000_0000;
                const WCLONE    = 0x4000_0000;
                const WALL      = 0x8000_0000;
            }
        }
        let opts = WaitIdOptions::from_bits_truncate(options);
        let nohang = opts.contains(WaitIdOptions::WNOHANG);
        let reap = !opts.contains(WaitIdOptions::WNOWAIT);
        let caller = self.zircon_process();

        let res = match idtype {
            P_PID => {
                if id == 0 {
                    return Err(LxError::EINVAL);
                }
                match wait_child(caller, id as KoID, nohang, reap).await {
                    Ok((code, _cpu)) => Ok((id as KoID, code)),
                    Err(LxError::EAGAIN) if nohang => Ok((0, 0)),
                    Err(e) => Err(e),
                }
            }
            P_PIDFD => {
                let pidfd = PidFd::from_file_like(self.linux_process().get_file_like(id.into())?)?;
                let target = pidfd.target();
                if !is_child_process(caller, target) {
                    return Err(LxError::ECHILD);
                }
                if FileLike::flags(pidfd.as_ref()).non_block()
                    && !matches!(target.status(), Status::Exited(_))
                    && !nohang
                {
                    return Err(LxError::EAGAIN);
                }
                match wait_child(caller, target.id(), nohang, reap).await {
                    Ok((code, _cpu)) => Ok((target.id(), code)),
                    Err(LxError::EAGAIN) if nohang => Ok((0, 0)),
                    Err(e) => Err(e),
                }
            }
            P_ALL => match wait_child_any(caller, nohang, reap).await {
                Ok((pid, code, _cpu)) => Ok((pid, code)),
                Err(LxError::EAGAIN) if nohang => Ok((0, 0)),
                Err(e) => Err(e),
            },
            P_PGID => return Err(LxError::ENOSYS),
            _ => return Err(LxError::EINVAL),
        };

        let (child_pid, status) = res?;

        if opts.contains(WaitIdOptions::WEXITED) || options == 0 {
            let exit_status = status >> 8;
            write_sigchld_info(infop, child_pid, exit_status)?;
        }
        Ok(0)
    }

    /// `sys_execve` executes the program referred to by `path`
    /// (see [linux man execve(2)](https://www.man7.org/linux/man-pages/man2/execve.2.html)).
    /// This causes the program that is currently being run
    /// by the calling process to be replaced with a new program,
    /// with newly initialized stack, heap, and (initialized and uninitialized) data segments.
    ///
    /// `path` argument must be a binary executable file.
    ///
    /// `argv` is an array of argument strings passed to the new program.
    /// By convention, the first of these strings (i.e., `argv[0]`)
    /// should contain the filename associated with the file being executed.
    ///
    /// `envp` is an array of strings, conventionally of the form `key=value`,
    /// which are passed as environment to the new program.
    ///
    /// > **NOTE!** Differ from linux, `argv` & `envp` can not be NULL.
    ///
    /// > **NOTE!** For multi-thread programs,
    /// > A call to any exec function from a process with more than one thread
    /// > shall result in all threads being terminated and the new executable image
    /// > being loaded and executed.
    pub fn sys_execve(
        &mut self,
        path: UserInPtr<u8>,
        argv: UserInPtr<UserInPtr<u8>>,
        envp: UserInPtr<UserInPtr<u8>>,
    ) -> SysResult {
        let path_str = path.as_c_str().inspect_err(|&e| {
            error!("execve: path.as_c_str() failed: {:?}", e);
        })?;
        // Normal program launch — keep at debug so shells / fork+exec loops
        // don't pay a synchronous serial write per exec at the default LOG=warn.
        debug!("EXECVE: path={:?}", path_str);
        let args = argv.read_cstring_array().inspect_err(|&e| {
            error!("execve: argv.read_cstring_array() failed: {:?}", e);
        })?;
        let mut envs: Vec<String> = Vec::new();
        if !envp.is_null() {
            envs = envp.read_cstring_array().inspect_err(|&e| {
                error!("execve: envp.read_cstring_array() failed: {:?}", e);
            })?;
        }
        info!(
            "execve: path: {:?}, args: {:?}, envs: {:?}",
            path_str, args, envs
        );
        // Record every program launch in the dmesg ring (path + argv) so a
        // captured trace shows the exact exec chain — crucially whether the X
        // server (`Xorg`/`X`/`Xorg.wrap`) is ever invoked and with what
        // arguments. `info!`/`debug!` are filtered out at the default WARN
        // level and never reach the ring, so use the unconditional klog path.
        kernel_hal::klog_info!(
            "EXECVE[{}] {:?} argv={:?}",
            self.zircon_process().id(),
            path_str,
            args
        );
        if args.is_empty() {
            error!("execve: args is empty");
            return Err(LxError::EINVAL);
        }
        if args[0].is_empty() {
            warn!("execve: argv[0] is empty for path {:?}", path_str);
        }

        // TODO: check and kill other threads

        // Read program file
        let proc = self.linux_process();
        // ROOT-CAUSE FIX for the "intermittent" kernel corruption (see
        // docs/README-crash-repro.md). busybox in standalone-shell mode
        // re-executes its applets via execve("/proc/self/exe"). Storing that
        // LITERAL string as the new image's `execute_path` makes the magic
        // link self-referential: the next lookup of "/proc/self/exe" in that
        // process (an open, a readlink, or — fatally — the execve of the NEXT
        // applet, e.g. busybox `timeout` spawning `sleep`) resolves
        // execute_path == "/proc/self/exe" and recurses in `lookup_inode_at`
        // without bound. The coroutine stack is a guard-page-less heap
        // allocation (currently 2 MiB usable + soft canary), so the runaway
        // recursion silently writes thousands of
        // stack frames DOWNWARD over neighbouring heap allocations — spraying
        // return addresses (0xffffff00...), ASCII "/proc/self/exe" bytes and
        // small values over live pointers. That is the wild-write signature
        // behind the `timeout -s TERM 1 sleep 5` triple fault, the mangled
        // #GP RIPs, the 0x87 MNode vtable and the `cat /proc/self/exe` hang.
        // Fix: canonicalize the magic link NOW — the current execute_path is
        // by construction the real on-disk binary path — so the literal
        // "/proc/self/exe" is never stored.
        let path_str = if path_str == "/proc/self/exe" {
            let real = proc.execute_path();
            if real.is_empty() {
                return Err(LxError::ENOENT);
            }
            alloc::borrow::Cow::Owned(real)
        } else {
            alloc::borrow::Cow::Borrowed(path_str)
        };
        let path_str: &str = &path_str;
        let inode = proc.lookup_inode(path_str)?;
        let metadata = inode.metadata()?;
        proc.check_access(&metadata, 0o1, true)?;
        let vmo = inode.read_as_vmo_cached()?;

        proc.remove_cloexec_files();
        // POSIX: caught signals are reset to their default disposition across
        // exec (SIG_IGN stays). Otherwise the child keeps inherited handler
        // addresses and a delivered signal jumps into stale handler code.
        proc.reset_signal_actions_for_exec();

        // 注意！即将销毁旧应用程序的用户空间，现在将必要的信息拷贝到内核！
        // Notice! About to destroy the user space of the old application, now copy the necessary information into kernel!
        let path_str = path_str.to_string();
        let vmar = self.zircon_process().vmar();
        vmar.clear()?;

        let (entry, sp, initial_brk, execute_path, abi) = LinuxElfLoader {
            syscall_entry: self.syscall_entry,
            stack_pages: USER_STACK_PAGES,
            root_inode: proc.root_inode().clone(),
        }
        .load(&vmar, &vmo, args.clone(), envs.clone(), path_str)
        .inspect_err(|&e| {
            error!("execve: LinuxElfLoader::load failed: {:?}", e);
        })?;
        // The new image may speak a different ABI than the caller (e.g. a Linux
        // shell exec'ing a FreeBSD binary); adopt the freshly-detected one.
        proc.set_abi(abi);
        proc.set_execute_path(&execute_path);
        proc.set_cmdline(args);
        proc.set_environ(envs);
        proc.set_brk(initial_brk);
        // CRUCIAL: reset the heap's *mapped* upper bound too. `execve` replaced
        // the whole address space (`vmar.clear()` above), so the previous image's
        // heap-chunk reservation is gone. Leaving `mapped_brk` stale (e.g.
        // 0x8d1000 from the old image) makes the next `sys_brk` believe the chunk
        // is still "within the reserved mapping" and skip the real `map_at` — so
        // musl writes into an unmapped heap and SIGSEGVs. This is the
        // deterministic `sh` crash in musl mallocng's __malloc_alloc_meta seen
        // when `sh -c gendepends.sh` re-execs into the script.
        proc.set_mapped_brk(initial_brk);
        proc.apply_exec_metadata(&metadata);
        self.zircon_process()
            .set_name(comm_from_path(&execute_path));
        // execve(2) resets the task's comm to the new executable's basename:
        // clearing the prctl(PR_SET_NAME) override makes /proc/<pid>/comm and
        // PR_GET_NAME fall back to exactly that. try_lock_linux: PID 1 / init can
        // re-exec (e.g. `exec`ing its real payload) and may carry no LinuxThread
        // ext; there is no comm override to clear in that case, so skip quietly
        // instead of unwrap-panicking.
        if let Some(mut lt) = self.thread.try_lock_linux() {
            lt.comm.clear();
        }
        // hunter: a new image is now in place — re-apply any default syscall
        // whitelist and reset the anomaly window so a benign-then-malicious
        // exec cannot launder accumulated detection state.
        hunter::task_exec(self.zircon_process().id(), &execute_path);

        self.zircon_process().signal_set(Signal::USER_SIGNAL_0);
        // FreeBSD/amd64 enters with a pointer to argc in %rdi and an 8-mod-16
        // stack (exec_setregs); Linux points %rsp at argc with cleared argument
        // registers.
        #[cfg(target_arch = "x86_64")]
        let (start_sp, arg0) = if abi == linux_object::process::Abi::Freebsd {
            (((sp - 8) & !0xf) + 8, sp)
        } else {
            (sp, 0)
        };
        #[cfg(not(target_arch = "x86_64"))]
        let (start_sp, arg0) = {
            let _ = abi;
            (sp, 0)
        };
        self.thread.with_context(|ctx| {
            *ctx = UserContext::new();
            ctx.setup_uspace(entry, start_sp, &[arg0, 0, 0]);
        })?;
        Ok(0)
    }

    //    pub fn sys_yield(&self) -> SysResult {
    //        thread::yield_now();
    //        Ok(0)
    //    }
    //

    /// `sys_gettid` returns the caller's thread ID (TID)
    /// (see [linux man gettid(2)](https://www.man7.org/linux/man-pages/man2/gettid.2.html)).
    /// In a single-threaded process, the thread ID is equal to the process ID (PID, as returned by [`Self::sys_getpid`]).
    /// In a multithreaded process, all threads have the same PID, but each one has a unique TID.
    pub fn sys_gettid(&self) -> SysResult {
        info!("gettid:");
        let tid = self.thread.id();
        Ok(tid as usize)
    }

    /// `sys_getpid` returns the process ID (PID) of the calling process
    /// (see [linux man getpid(2)](https://www.man7.org/linux/man-pages/man2/getpid.2.html)).
    pub fn sys_getpid(&self) -> SysResult {
        info!("getpid:");
        let proc = self.zircon_process();
        let pid = proc.id();
        Ok(pid as usize)
    }

    /// `sys_getppid` returns the process ID of the parent of the calling process
    /// (see [linux man getppid(2)](https://www.man7.org/linux/man-pages/man2/getpid.2.html)).
    /// This will be either the ID of the process that created this process using fork(),
    /// or, if that process has already terminated, 0.
    pub fn sys_getppid(&self) -> SysResult {
        info!("getppid:");
        let proc = self.linux_process();
        let ppid = proc.parent().map(|p| p.id()).unwrap_or(0);
        Ok(ppid as usize)
    }

    /// `sys_exit` system call terminates only the calling thread
    /// (see [linux man _exit(2)](https://www.man7.org/linux/man-pages/man2/exit.2.html),
    /// this syscall is same as a raw `_exit` in glibc),
    /// and actions such as reparenting child processes or sending
    /// SIGCHLD to the parent process are performed only if this is the
    /// last thread in the thread group.
    pub fn sys_exit(&mut self, exit_code: i32) -> SysResult {
        info!("exit: code={}", exit_code);
        self.thread.exit_linux(exit_code);
        Ok(0)
    }

    /// `sys_exit_group` is equivalent to [`Self::sys_exit`]
    /// except that it terminates not only the calling thread
    /// (see [linux man exit_group(2)](https://www.man7.org/linux/man-pages/man2/exit_group.2.html),
    /// but all threads in the calling process's thread group.
    /// As a result, the entire calling process will exit.
    pub fn sys_exit_group(&mut self, exit_code: i32) -> SysResult {
        info!("exit_group: code={}", exit_code);
        let proc = self.zircon_process();
        // hunter state is released centrally in zircon Process::terminate so it
        // covers every teardown path (signal-kill, exception, _exit), not just
        // exit_group — see zircon-object/src/task/process.rs.
        proc.exit(exit_code as i64);
        Ok(0)
    }

    /// Allows the calling thread to sleep for
    /// an interval specified with nanosecond precision
    /// (see [linux man nanosleep(2)](https://www.man7.org/linux/man-pages/man2/nanosleep.2.html).
    ///
    /// `nanosleep` suspends the execution of the calling thread
    /// until either at least the time specified in `req` has elapsed,
    /// or the delivery of a signal that triggers the invocation of a handler
    /// in the calling thread or that terminates the process.
    ///
    /// To represent a duration, see TimeSpec.
    pub async fn sys_nanosleep(&self, req: UserInPtr<TimeSpec>) -> SysResult {
        info!("nanosleep: deadline={:?}", req);
        let duration = req.read()?.into();
        let deadline = kernel_hal::timer::deadline_after(duration);
        // Check for pending signals before blocking.
        linux_object::process::check_signals()?;
        if kernel_hal::timer::timer_now() >= deadline {
            return Ok(0);
        }
        // Sleep efficiently until the deadline instead of spinning with
        // yield_now(). This eliminates per-tick rescheduling noise for all
        // sleeping tasks, which was the primary source of scheduler lag.
        // A signal check after wakeup preserves EINTR semantics for signals
        // that arrive while the task is dormant.
        kernel_hal::thread::sleep_until(deadline).await;
        linux_object::process::check_signals()?;
        Ok(0)
    }

    //    pub fn sys_set_priority(&self, priority: usize) -> SysResult {
    //        let pid = thread::current().id();
    //        thread_manager().set_priority(pid, priority as u8);
    //        Ok(0)
    //    }

    /// Bitmask of CPUs that are currently online (logical ids that reached
    /// `secondary_init`). Prefer HAL's online bitset over `(1<<cpu_count())-1`,
    /// which includes APs that never came up.
    fn online_cpu_mask() -> u64 {
        kernel_hal::cpu_online_mask()
    }

    /// Resolve the thread targeted by a `sched_*affinity` call.
    ///
    /// Per Linux semantics the `pid` argument is a TID. `pid == 0` (handled by
    /// the callers) means the calling thread. Otherwise we search every live
    /// process for a thread whose TID matches; failing that we treat `pid` as a
    /// process id and return its leader thread, so `taskset -p <pid>` works for
    /// single-threaded processes.
    fn find_thread_by_tid(&self, pid: usize) -> Option<Arc<Thread>> {
        let id = pid as KoID;
        for proc in linux_object::process::all_live_processes() {
            if let Ok(obj) = proc.get_child(id) {
                if let Ok(thread) = obj.downcast_arc::<Thread>() {
                    return Some(thread);
                }
            }
        }
        let proc = zircon_object::task::ROOT_JOB.find_process(id)?;
        let first = *proc.thread_ids().first()?;
        proc.get_child(first).ok()?.downcast_arc::<Thread>().ok()
    }

    /// `sched_setaffinity` sets the CPU affinity mask of the thread `pid`.
    ///
    /// The mask is masked down to the set of online CPUs; an empty effective
    /// mask is rejected with `EINVAL`. See
    /// [linux man sched_setaffinity(2)](https://www.man7.org/linux/man-pages/man2/sched_setaffinity.2.html).
    pub fn sys_sched_setaffinity(
        &self,
        pid: usize,
        cpusetsize: usize,
        mask_ptr: UserInPtr<u8>,
    ) -> SysResult {
        info!(
            "sched_setaffinity: pid={} cpusetsize={} mask_ptr={:?}",
            pid, cpusetsize, mask_ptr
        );
        if cpusetsize == 0 {
            return Err(LxError::EINVAL);
        }
        // Only the low 64 CPUs are representable (MAX_CORE_NUM == 64).
        let n = cpusetsize.min(8);
        let bytes = mask_ptr.read_array(n)?;
        let mut mask = 0u64;
        for (i, b) in bytes.iter().enumerate() {
            mask |= (*b as u64) << (i * 8);
        }
        let eff = mask & Self::online_cpu_mask();
        if eff == 0 {
            return Err(LxError::EINVAL);
        }
        if pid == 0 || pid as u64 == self.thread.id() {
            self.thread.set_affinity(eff).map_err(|_| LxError::EINVAL)?;
        } else {
            let thread = self.find_thread_by_tid(pid).ok_or(LxError::ESRCH)?;
            thread.set_affinity(eff).map_err(|_| LxError::EINVAL)?;
        }
        Ok(0)
    }

    /// `sched_getaffinity` writes the CPU affinity mask of the thread `pid`
    /// into the user buffer and returns the number of bytes written.
    ///
    /// See [linux man sched_getaffinity(2)](https://www.man7.org/linux/man-pages/man2/sched_getaffinity.2.html).
    pub fn sys_sched_getaffinity(
        &self,
        pid: usize,
        cpusetsize: usize,
        mut mask_ptr: UserOutPtr<u8>,
    ) -> SysResult {
        info!(
            "sched_getaffinity: pid={} cpusetsize={} mask_ptr={:?}",
            pid, cpusetsize, mask_ptr
        );
        if cpusetsize == 0 {
            return Err(LxError::EINVAL);
        }
        let mask = if pid == 0 || pid as u64 == self.thread.id() {
            self.thread.affinity()
        } else {
            self.find_thread_by_tid(pid)
                .ok_or(LxError::ESRCH)?
                .affinity()
        } & Self::online_cpu_mask();
        // The kernel cpumask is 8 bytes wide for up to 64 CPUs; copy out at most
        // that many (libc zero-fills any remaining bytes of its cpu_set_t).
        let n = cpusetsize.min(8);
        let bytes = mask.to_le_bytes();
        mask_ptr.write_array(&bytes[..n])?;
        Ok(n)
    }

    /// Resolve the thread targeted by a `sched_*` / `*priority` call.
    ///
    /// `pid == 0` (or the caller's own TID) selects the calling thread; any
    /// other value is looked up with [`find_thread_by_tid`](Self::find_thread_by_tid),
    /// which also accepts a process id for single-threaded programs. Missing
    /// targets yield `ESRCH`, matching Linux.
    fn sched_target(&self, pid: usize) -> Result<Arc<Thread>, LxError> {
        if pid == 0 || pid as u64 == self.thread.id() {
            Ok(self.thread.inner())
        } else {
            self.find_thread_by_tid(pid).ok_or(LxError::ESRCH)
        }
    }

    /// Validate a `(policy, sched_priority, nice)` triple against Linux's rules
    /// and return the normalised `(policy, rt_priority, nice)` to store.
    ///
    /// Real-time policies require `sched_priority` in `1..=99`; the fair
    /// policies require it to be `0`. The nice value is clamped to `-20..=19`.
    fn sched_validate(policy: u8, sched_priority: i32, nice: i32) -> Result<(u8, u8, i8), LxError> {
        let nice = nice.clamp(MIN_NICE as i32, MAX_NICE as i32) as i8;
        match policy {
            SCHED_FIFO | SCHED_RR => {
                if !(MIN_RT_PRIO as i32..=MAX_RT_PRIO as i32).contains(&sched_priority) {
                    return Err(LxError::EINVAL);
                }
                Ok((policy, sched_priority as u8, nice))
            }
            SCHED_NORMAL | SCHED_BATCH | SCHED_IDLE => {
                if sched_priority != 0 {
                    return Err(LxError::EINVAL);
                }
                Ok((policy, 0, nice))
            }
            _ => Err(LxError::EINVAL),
        }
    }

    /// `(min, max)` `sched_priority` for `policy`, or `EINVAL` for an unknown one.
    fn rt_priority_bounds(policy: usize) -> Result<(u8, u8), LxError> {
        if policy == SCHED_FIFO as usize || policy == SCHED_RR as usize {
            Ok((MIN_RT_PRIO, MAX_RT_PRIO))
        } else if policy == SCHED_NORMAL as usize
            || policy == SCHED_BATCH as usize
            || policy == SCHED_IDLE as usize
        {
            Ok((0, 0))
        } else {
            Err(LxError::EINVAL)
        }
    }

    /// `sched_setscheduler` sets both the scheduling policy and (real-time)
    /// priority of thread `pid`. The thread's nice value is preserved.
    ///
    /// See [linux man sched_setscheduler(2)](https://www.man7.org/linux/man-pages/man2/sched_setscheduler.2.html).
    pub fn sys_sched_setscheduler(
        &self,
        pid: usize,
        policy: usize,
        param: UserInPtr<i32>,
    ) -> SysResult {
        // The high SCHED_RESET_ON_FORK bit is accepted but not modelled.
        let base = policy & !SCHED_RESET_ON_FORK;
        if base > u8::MAX as usize {
            return Err(LxError::EINVAL);
        }
        let sched_priority = param.read()?;
        info!(
            "sched_setscheduler: pid={} policy={} priority={}",
            pid, base, sched_priority
        );
        let thread = self.sched_target(pid)?;
        let (p, rt, nice) =
            Self::sched_validate(base as u8, sched_priority, thread.sched_nice() as i32)?;
        thread.set_sched(p, nice, rt);
        Ok(0)
    }

    /// `sched_getscheduler` returns the scheduling policy of thread `pid`.
    ///
    /// See [linux man sched_getscheduler(2)](https://www.man7.org/linux/man-pages/man2/sched_getscheduler.2.html).
    pub fn sys_sched_getscheduler(&self, pid: usize) -> SysResult {
        let thread = self.sched_target(pid)?;
        Ok(thread.sched_policy() as usize)
    }

    /// `sched_setparam` sets the (real-time) priority of thread `pid` without
    /// changing its policy.
    ///
    /// See [linux man sched_setparam(2)](https://www.man7.org/linux/man-pages/man2/sched_setparam.2.html).
    pub fn sys_sched_setparam(&self, pid: usize, param: UserInPtr<i32>) -> SysResult {
        let sched_priority = param.read()?;
        let thread = self.sched_target(pid)?;
        let (p, rt, nice) = Self::sched_validate(
            thread.sched_policy(),
            sched_priority,
            thread.sched_nice() as i32,
        )?;
        thread.set_sched(p, nice, rt);
        Ok(0)
    }

    /// `sched_getparam` writes the (real-time) priority of thread `pid` into the
    /// user-supplied `struct sched_param`.
    ///
    /// See [linux man sched_getparam(2)](https://www.man7.org/linux/man-pages/man2/sched_getparam.2.html).
    pub fn sys_sched_getparam(&self, pid: usize, mut param: UserOutPtr<i32>) -> SysResult {
        let thread = self.sched_target(pid)?;
        param.write(thread.sched_rt_priority() as i32)?;
        Ok(0)
    }

    /// `sched_get_priority_max` returns the maximum `sched_priority` usable with
    /// `policy` (99 for FIFO/RR, 0 for the fair policies).
    pub fn sys_sched_get_priority_max(&self, policy: usize) -> SysResult {
        Ok(Self::rt_priority_bounds(policy)?.1 as usize)
    }

    /// `sched_get_priority_min` returns the minimum `sched_priority` usable with
    /// `policy` (1 for FIFO/RR, 0 for the fair policies).
    pub fn sys_sched_get_priority_min(&self, policy: usize) -> SysResult {
        Ok(Self::rt_priority_bounds(policy)?.0 as usize)
    }

    /// `sched_rr_get_interval` writes the round-robin timeslice of thread `pid`
    /// into `interval`. Non-`SCHED_RR` threads report a zero interval.
    ///
    /// See [linux man sched_rr_get_interval(2)](https://www.man7.org/linux/man-pages/man2/sched_rr_get_interval.2.html).
    pub fn sys_sched_rr_get_interval(
        &self,
        pid: usize,
        mut interval: UserOutPtr<TimeSpec>,
    ) -> SysResult {
        let thread = self.sched_target(pid)?;
        let ts = if thread.sched_policy() == SCHED_RR {
            TimeSpec {
                sec: 0,
                nsec: 100_000_000,
            }
        } else {
            TimeSpec { sec: 0, nsec: 0 }
        };
        interval.write(ts)?;
        Ok(0)
    }

    /// `sched_setattr` sets policy, nice and real-time priority of thread `pid`
    /// from a `struct sched_attr`. `SCHED_DEADLINE` is rejected (`EINVAL`) since
    /// this scheduler has no deadline runqueue.
    ///
    /// See [linux man sched_setattr(2)](https://www.man7.org/linux/man-pages/man2/sched_setattr.2.html).
    pub fn sys_sched_setattr(
        &self,
        pid: usize,
        attr: UserInPtr<SchedAttr>,
        flags: usize,
    ) -> SysResult {
        if flags != 0 {
            return Err(LxError::EINVAL);
        }
        let a = attr.read()?;
        if (a.size as usize) < size_of::<SchedAttr>() {
            return Err(LxError::EINVAL);
        }
        let policy = (a.sched_policy & !(SCHED_RESET_ON_FORK as u32)) as usize;
        if policy == SCHED_DEADLINE as usize || policy > u8::MAX as usize {
            return Err(LxError::EINVAL);
        }
        info!(
            "sched_setattr: pid={} policy={} nice={}",
            pid, policy, a.sched_nice
        );
        let thread = self.sched_target(pid)?;
        let (p, rt, nice) =
            Self::sched_validate(policy as u8, a.sched_priority as i32, a.sched_nice)?;
        thread.set_sched(p, nice, rt);
        Ok(0)
    }

    /// `sched_getattr` writes thread `pid`'s scheduling attributes into a
    /// caller-supplied `struct sched_attr` of `size` bytes.
    ///
    /// See [linux man sched_getattr(2)](https://www.man7.org/linux/man-pages/man2/sched_getattr.2.html).
    pub fn sys_sched_getattr(
        &self,
        pid: usize,
        mut attr: UserOutPtr<SchedAttr>,
        size: usize,
        flags: usize,
    ) -> SysResult {
        if flags != 0 {
            return Err(LxError::EINVAL);
        }
        if size < size_of::<SchedAttr>() {
            return Err(LxError::EINVAL);
        }
        let thread = self.sched_target(pid)?;
        let a = SchedAttr {
            size: size_of::<SchedAttr>() as u32,
            sched_policy: thread.sched_policy() as u32,
            sched_flags: 0,
            sched_nice: thread.sched_nice() as i32,
            sched_priority: thread.sched_rt_priority() as u32,
            sched_runtime: 0,
            sched_deadline: 0,
            sched_period: 0,
        };
        attr.write(a)?;
        Ok(0)
    }

    /// `setpriority` sets the nice value of the target selected by `which`/`who`.
    ///
    /// Only `PRIO_PROCESS` resolves to a specific task; `PRIO_PGRP` / `PRIO_USER`
    /// are accepted and applied to the calling thread so `nice(1)` / `renice`
    /// behave sensibly. `prio` is clamped to the nice range `-20..=19`.
    ///
    /// See [linux man setpriority(2)](https://www.man7.org/linux/man-pages/man2/setpriority.2.html).
    pub fn sys_setpriority(&self, which: usize, who: usize, prio: i32) -> SysResult {
        if which != PRIO_PROCESS && which != PRIO_PGRP && which != PRIO_USER {
            return Err(LxError::EINVAL);
        }
        let nice = prio.clamp(MIN_NICE as i32, MAX_NICE as i32) as i8;
        let thread = if which == PRIO_PROCESS {
            self.sched_target(who)?
        } else {
            self.thread.inner()
        };
        info!("setpriority: which={} who={} nice={}", which, who, nice);
        thread.set_sched(thread.sched_policy(), nice, thread.sched_rt_priority());
        Ok(0)
    }

    /// `getpriority` returns the nice value of the target selected by
    /// `which`/`who`, encoded as `20 - nice` so the raw syscall return stays
    /// non-negative (glibc converts it back to the user-visible nice value).
    ///
    /// See [linux man getpriority(2)](https://www.man7.org/linux/man-pages/man2/getpriority.2.html).
    pub fn sys_getpriority(&self, which: usize, who: usize) -> SysResult {
        if which != PRIO_PROCESS && which != PRIO_PGRP && which != PRIO_USER {
            return Err(LxError::EINVAL);
        }
        let thread = if which == PRIO_PROCESS {
            self.sched_target(who)?
        } else {
            self.thread.inner()
        };
        Ok((20 - thread.sched_nice() as isize) as usize)
    }

    /// `set_tid_address` sets the clear_child_tid value for the calling thread to `tidptr`,
    /// and return the caller's thread ID
    /// (see [linux man set_tid_address(2)](https://www.man7.org/linux/man-pages/man2/set_tid_address.2.html).
    pub fn sys_set_tid_address(&self, tidptr: UserOutPtr<i32>) -> SysResult {
        info!("set_tid_address: {:?}", tidptr);
        self.thread.set_tid_address(tidptr);
        let tid = self.thread.id();
        Ok(tid as usize)
    }

    /// Get robust list.
    pub fn sys_get_robust_list(
        &self,
        pid: i32,
        head_ptr: UserOutPtr<UserOutPtr<RobustList>>,
        len_ptr: UserOutPtr<usize>,
    ) -> SysResult {
        if pid == 0 {
            return self.thread.get_robust_list(head_ptr, len_ptr);
        }
        Ok(0)
    }

    /// Set robust list.
    pub fn sys_set_robust_list(&self, head: UserInPtr<RobustList>, len: usize) -> SysResult {
        if len != size_of::<RobustList>() {
            return Err(LxError::EINVAL);
        }
        self.thread.set_robust_list(head, len);
        Ok(0)
    }

    /// `getuid` returns the real user ID of the calling process.
    pub fn sys_getuid(&self) -> SysResult {
        debug!("getuid");
        Ok(self.linux_process().uid() as usize)
    }

    /// `geteuid` returns the effective user ID of the calling process.
    pub fn sys_geteuid(&self) -> SysResult {
        debug!("geteuid");
        Ok(self.linux_process().euid() as usize)
    }

    /// `getgid` returns the real group ID of the calling process.
    pub fn sys_getgid(&self) -> SysResult {
        debug!("getgid");
        Ok(self.linux_process().gid() as usize)
    }

    /// `getegid` returns the effective group ID of the calling process.
    pub fn sys_getegid(&self) -> SysResult {
        debug!("getegid");
        Ok(self.linux_process().egid() as usize)
    }

    /// `umask` updates and returns the previous creation mask.
    pub fn sys_umask(&self, mask: usize) -> SysResult {
        Ok(self.linux_process().set_umask(mask as u16) as usize)
    }

    /// `setuid` changes the calling process user identity.
    pub fn sys_setuid(&self, uid: usize) -> SysResult {
        self.linux_process().set_uid(uid as u32)?;
        Ok(0)
    }

    /// `setgid` changes the calling process group identity.
    pub fn sys_setgid(&self, gid: usize) -> SysResult {
        self.linux_process().set_gid(gid as u32)?;
        Ok(0)
    }

    /// `setreuid` changes the real/effective user IDs.
    pub fn sys_setreuid(&self, ruid: usize, euid: usize) -> SysResult {
        self.linux_process().set_reuid(ruid as u32, euid as u32)?;
        Ok(0)
    }

    /// `setregid` changes the real/effective group IDs.
    pub fn sys_setregid(&self, rgid: usize, egid: usize) -> SysResult {
        self.linux_process().set_regid(rgid as u32, egid as u32)?;
        Ok(0)
    }

    /// `setresuid` changes the real/effective/saved user IDs.
    pub fn sys_setresuid(&self, ruid: usize, euid: usize, suid: usize) -> SysResult {
        self.linux_process()
            .set_resuid(ruid as u32, euid as u32, suid as u32)?;
        Ok(0)
    }

    /// `setresgid` changes the real/effective/saved group IDs.
    pub fn sys_setresgid(&self, rgid: usize, egid: usize, sgid: usize) -> SysResult {
        self.linux_process()
            .set_resgid(rgid as u32, egid as u32, sgid as u32)?;
        Ok(0)
    }

    /// `getgroups` returns supplementary group IDs.
    pub fn sys_getgroups(&self, size: usize, mut list: UserOutPtr<u32>) -> SysResult {
        let groups = self.linux_process().groups();
        if size == 0 {
            return Ok(groups.len());
        }
        if size < groups.len() {
            return Err(LxError::EINVAL);
        }
        list.write_array(groups.as_slice())?;
        Ok(groups.len())
    }

    /// `setgroups` updates supplementary group IDs.
    pub fn sys_setgroups(&self, size: usize, list: UserInPtr<u32>) -> SysResult {
        if !self.linux_process().is_superuser() {
            return Err(LxError::EPERM);
        }
        let groups = if size == 0 {
            Vec::new()
        } else {
            list.read_array(size)?
        };
        self.linux_process().set_groups(groups);
        Ok(0)
    }

    /// `setpgid` sets the PGID of the process specified by pid to pgid.
    /// `pid == 0` targets the caller; `pgid == 0` makes the target its own
    /// group leader. Job-control shells rely on this to put each foreground job
    /// into its own process group so a Ctrl-C reaches the job, not the shell.
    pub fn sys_setpgid(&self, pid: usize, pgid: usize) -> SysResult {
        debug!("setpgid: pid={}, pgid={}", pid, pgid);
        let target = if pid == 0 {
            self.zircon_process().id()
        } else {
            pid as u64
        };
        let new_pgid = if pgid == 0 { target } else { pgid as u64 };
        linux_object::process::set_process_pgid(target, new_pgid)?;
        Ok(0)
    }

    /// `getpgid` returns the PGID of the process specified by pid.
    pub fn sys_getpgid(&self, pid: usize) -> SysResult {
        debug!("getpgid: pid={}", pid);
        let target = if pid == 0 {
            self.zircon_process().id()
        } else {
            pid as u64
        };
        let pgid = linux_object::process::get_process_pgid(target)?;
        Ok(pgid as usize)
    }

    /// `setsid` creates a new session if the calling process is not a process
    /// group leader: the caller becomes leader of a new session and of a new
    /// process group, both ids equal to its pid (see setsid(2)).
    pub fn sys_setsid(&self) -> SysResult {
        let pid = self.zircon_process().id();
        let proc = self.linux_process();
        // POSIX: a process-group leader may not create a new session (its pid
        // already names an existing group). The daemonize idiom fork()s first
        // precisely so the child is not a leader.
        let pgid = proc.pgid_raw();
        if pgid == 0 || pgid == pid {
            debug!("setsid: pid {} is already a group leader", pid);
            return Err(LxError::EPERM);
        }
        proc.become_session_leader(pid);
        info!("setsid: pid {} starts a new session", pid);
        Ok(pid as usize)
    }

    /// `getsid` returns the session ID of the process specified by pid
    /// (0 = the calling process); see getsid(2).
    pub fn sys_getsid(&self, pid: usize) -> SysResult {
        debug!("getsid: pid={}", pid);
        let target = if pid == 0 {
            self.zircon_process().id()
        } else {
            pid as u64
        };
        let sid = linux_object::process::get_process_sid(target)?;
        Ok(sid as usize)
    }

    /// Operations on a process or thread (see prctl(2) and, for
    /// `PR_SET_NO_NEW_PRIVS`, Documentation/userspace-api/no_new_privs.rst).
    ///
    /// The options below are implemented against real per-process/per-thread
    /// state; anything else is refused with `EINVAL`, exactly like a kernel
    /// that does not know the option. (The previous behaviour — returning 0
    /// for *every* option — silently claimed e.g. seccomp had been engaged.)
    pub fn sys_prctl(&self, option: i32, a2: usize, a3: usize, a4: usize, a5: usize) -> SysResult {
        use linux_object::signal::Signal as LinuxSignal;
        use linux_object::thread::TASK_COMM_LEN;

        const PR_SET_PDEATHSIG: i32 = 1;
        const PR_GET_PDEATHSIG: i32 = 2;
        const PR_GET_DUMPABLE: i32 = 3;
        const PR_SET_DUMPABLE: i32 = 4;
        const PR_SET_NAME: i32 = 15;
        const PR_GET_NAME: i32 = 16;
        const PR_GET_SECCOMP: i32 = 21;
        const PR_SET_SECCOMP: i32 = 22;
        const PR_CAPBSET_READ: i32 = 23;
        const PR_CAPBSET_DROP: i32 = 24;
        const PR_SET_TIMERSLACK: i32 = 29;
        const PR_GET_TIMERSLACK: i32 = 30;
        const PR_SET_CHILD_SUBREAPER: i32 = 36;
        const PR_GET_CHILD_SUBREAPER: i32 = 37;
        const PR_SET_NO_NEW_PRIVS: i32 = 38;
        const PR_GET_NO_NEW_PRIVS: i32 = 39;
        const PR_GET_TID_ADDRESS: i32 = 40;
        const PR_SET_THP_DISABLE: i32 = 41;
        const PR_GET_THP_DISABLE: i32 = 42;
        /// Highest capability number this kernel reports (matches `capget`).
        const CAP_LAST_CAP: usize = 40;
        /// Default timer slack, ns (Linux: 50 µs for every fresh task).
        const TIMERSLACK_DEFAULT_NS: u64 = 50_000;

        debug!(
            "prctl: option={}, args={:#x},{:#x},{:#x},{:#x}",
            option, a2, a3, a4, a5
        );
        let proc = self.linux_process();
        match option {
            PR_SET_PDEATHSIG => {
                if a2 == 0 {
                    proc.set_pdeathsig(0);
                    return Ok(0);
                }
                // Only real, deliverable signal numbers may be latched.
                LinuxSignal::try_from(a2 as u8).map_err(|_| LxError::EINVAL)?;
                proc.set_pdeathsig(a2 as u8);
                Ok(0)
            }
            PR_GET_PDEATHSIG => {
                let mut out: UserOutPtr<i32> = a2.into();
                out.write(proc.pdeathsig() as i32)?;
                Ok(0)
            }
            PR_GET_DUMPABLE => Ok(proc.dumpable() as usize),
            PR_SET_DUMPABLE => {
                // prctl(2): since Linux 2.6.13 only SUID_DUMP_DISABLE (0) and
                // SUID_DUMP_USER (1) may be set this way.
                if a2 > 1 {
                    return Err(LxError::EINVAL);
                }
                proc.set_dumpable(a2 as u8);
                Ok(0)
            }
            PR_SET_NAME => {
                let name_ptr: UserInPtr<u8> = a2.into();
                let name = name_ptr.as_c_str()?;
                let mut comm = alloc::string::String::new();
                // TASK_COMM_LEN includes the NUL: keep at most 15 bytes.
                for c in name.chars() {
                    if comm.len() + c.len_utf8() > TASK_COMM_LEN - 1 {
                        break;
                    }
                    comm.push(c);
                }
                self.thread.lock_linux().comm = comm;
                Ok(0)
            }
            PR_GET_NAME => {
                let comm = self.thread.lock_linux().comm.clone();
                // Fall back to the executable's basename, mirroring what
                // /proc/<pid>/comm reports for a never-named thread.
                let name = if comm.is_empty() {
                    let path = proc.execute_path();
                    path.rsplit('/').next().unwrap_or_default().to_string()
                } else {
                    comm
                };
                // The buffer is specified to hold at least 16 bytes; write the
                // name NUL-terminated and NUL-padded like the kernel does.
                let mut buf = [0u8; TASK_COMM_LEN];
                let bytes = name.as_bytes();
                let n = bytes.len().min(TASK_COMM_LEN - 1);
                buf[..n].copy_from_slice(&bytes[..n]);
                let mut out: UserOutPtr<u8> = a2.into();
                out.write_array(&buf)?;
                Ok(0)
            }
            // No seccomp machinery: answer exactly like a kernel built
            // without CONFIG_SECCOMP.
            PR_GET_SECCOMP | PR_SET_SECCOMP => Err(LxError::EINVAL),
            PR_CAPBSET_READ => {
                if a2 > CAP_LAST_CAP {
                    return Err(LxError::EINVAL);
                }
                // Root-run kernel: every valid capability is in the bounding
                // set (consistent with sys_capget).
                Ok(1)
            }
            PR_CAPBSET_DROP => {
                if a2 > CAP_LAST_CAP {
                    return Err(LxError::EINVAL);
                }
                // There is no stored bounding set to shrink; accepting keeps
                // privilege-dropping daemons on their happy path, consistent
                // with sys_capset.
                Ok(0)
            }
            PR_SET_TIMERSLACK => {
                self.thread.lock_linux().timerslack_ns = a2 as u64;
                Ok(0)
            }
            PR_GET_TIMERSLACK => {
                let slack = self.thread.lock_linux().timerslack_ns;
                Ok(if slack == 0 {
                    TIMERSLACK_DEFAULT_NS as usize
                } else {
                    slack as usize
                })
            }
            PR_SET_CHILD_SUBREAPER => {
                proc.set_child_subreaper(a2 != 0);
                Ok(0)
            }
            PR_GET_CHILD_SUBREAPER => {
                let mut out: UserOutPtr<i32> = a2.into();
                out.write(proc.is_child_subreaper() as i32)?;
                Ok(0)
            }
            PR_SET_NO_NEW_PRIVS => {
                // prctl(2): arg2 must be 1 (the flag can never be cleared) and
                // the remaining arguments must be zero.
                if a2 != 1 || a3 != 0 || a4 != 0 || a5 != 0 {
                    return Err(LxError::EINVAL);
                }
                proc.set_no_new_privs();
                Ok(0)
            }
            PR_GET_NO_NEW_PRIVS => {
                if a2 != 0 || a3 != 0 || a4 != 0 || a5 != 0 {
                    return Err(LxError::EINVAL);
                }
                Ok(proc.no_new_privs() as usize)
            }
            PR_GET_TID_ADDRESS => {
                let mut out: UserOutPtr<usize> = a2.into();
                out.write(self.thread.lock_linux().tid_address())?;
                Ok(0)
            }
            PR_SET_THP_DISABLE => {
                if a3 != 0 || a4 != 0 || a5 != 0 {
                    return Err(LxError::EINVAL);
                }
                proc.set_thp_disable(a2 != 0);
                Ok(0)
            }
            PR_GET_THP_DISABLE => Ok(proc.thp_disable() as usize),
            _ => {
                debug!("prctl: unknown option {}", option);
                Err(LxError::EINVAL)
            }
        }
    }

    /// `chmod` changes the mode of the file specified by path.
    pub fn sys_chmod(&self, path: UserInPtr<u8>, mode: usize) -> SysResult {
        let path = path.as_c_str()?;
        debug!("chmod: path={:?}, mode={:#o}", path, mode);
        let proc = self.linux_process();
        let inode = proc.lookup_inode(path)?;
        let mut metadata = inode.metadata()?;
        proc.chmod_metadata(&mut metadata, mode as u16)?;
        inode.set_metadata(&metadata)?;
        Ok(0)
    }

    /// `getresuid` returns the real, effective, and saved user IDs.
    pub fn sys_getresuid(
        &self,
        mut ruid: UserOutPtr<u32>,
        mut euid: UserOutPtr<u32>,
        mut suid: UserOutPtr<u32>,
    ) -> SysResult {
        debug!(
            "getresuid: ruid={:?}, euid={:?}, suid={:?}",
            ruid, euid, suid
        );
        let creds = self.linux_process().credentials();
        ruid.write(creds.ruid)?;
        euid.write(creds.euid)?;
        suid.write(creds.suid)?;
        Ok(0)
    }

    /// `getresgid` returns the real, effective, and saved group IDs.
    pub fn sys_getresgid(
        &self,
        mut rgid: UserOutPtr<u32>,
        mut egid: UserOutPtr<u32>,
        mut sgid: UserOutPtr<u32>,
    ) -> SysResult {
        debug!(
            "getresgid: rgid={:?}, egid={:?}, sgid={:?}",
            rgid, egid, sgid
        );
        let creds = self.linux_process().credentials();
        rgid.write(creds.rgid)?;
        egid.write(creds.egid)?;
        sgid.write(creds.sgid)?;
        Ok(0)
    }

    /// `setfsuid` sets the user ID used for filesystem checks.
    pub fn sys_setfsuid(&self, fsuid: usize) -> SysResult {
        debug!("setfsuid: fsuid={}", fsuid);
        let old_fsuid = self.linux_process().euid() as usize;
        Ok(old_fsuid)
    }

    /// `setfsgid` sets the group ID used for filesystem checks.
    pub fn sys_setfsgid(&self, fsgid: usize) -> SysResult {
        debug!("setfsgid: fsgid={}", fsgid);
        let old_fsgid = self.linux_process().egid() as usize;
        Ok(old_fsgid)
    }
}

bitflags! {
    pub struct CloneFlags: usize {
        ///
        const CSIGNAL =         0xff;
        /// the calling process and the child process run in the same memory space
        const VM =              1 << 8;
        /// the caller and the child process share the same filesystem information
        const FS =              1 << 9;
        /// the calling process and the child process share the same file descriptor table
        const FILES =           1 << 10;
        /// the calling process and the child process share the same table of signal handlers.
        const SIGHAND =         1 << 11;
        /// return a pidfd referring to the child process
        const PIDFD =           1 << 12;
        /// the calling process is being traced
        const PTRACE =          1 << 13;
        /// the execution of the calling process is suspended until the child releases its virtual memory resources
        const VFORK =           1 << 14;
        /// the parent of the new child will be the same as that of the call‐ing process.
        const PARENT =          1 << 15;
        /// the child is placed in the same thread group as the calling process.
        const THREAD =          1 << 16;
        /// cloned child is started in a new mount namespace
        const NEWNS	=           1 << 17;
        /// the child and the calling process share a single list of System V semaphore adjustment values.
        const SYSVSEM =         1 << 18;
        /// architecture dependent, The TLS (Thread Local Storage) descriptor is set to tls.
        const SETTLS =          1 << 19;
        /// Store the child thread ID at the location in the parent's memory.
        const PARENT_SETTID =   1 << 20;
        /// Clear (zero) the child thread ID
        const CHILD_CLEARTID =  1 << 21;
        /// the parent not to receive a signal when the child terminated
        const DETACHED =        1 << 22;
        /// a tracing process cannot force CLONE_PTRACE on this child process.
        const UNTRACED =        1 << 23;
        /// Store the child thread ID
        const CHILD_SETTID =    1 << 24;
        /// Create the process in a new cgroup namespace.
        const NEWCGROUP =       1 << 25;
        /// create the process in a new UTS namespace
        const NEWUTS =          1 << 26;
        /// create the process in a new IPC namespace.
        const NEWIPC =          1 << 27;
        /// create the process in a new user namespace
        const NEWUSER =         1 << 28;
        /// create the process in a new PID namespace
        const NEWPID =          1 << 29;
        /// create the process in a new net‐work namespace.
        const NEWNET =          1 << 30;
        /// the new process shares an I/O context with the calling process.
        const IO =              1 << 31;
    }
}
