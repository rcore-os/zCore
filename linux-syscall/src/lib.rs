//! Linux syscall implementations
//!
//! ## Example
//! The syscall is called like this in the [`zcore_loader`](../zcore_loader/index.html):
//! ```ignore
//! let num = regs.rax as u32;
//! let args = [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9];
//! let mut syscall = Syscall {
//!     thread,
//!     thread_fn,
//!     syscall_entry: kernel_hal::context::syscall_entry as usize,
//! };
//! let ret = syscall.syscall(num, args).await;
//! ```
//!

#![no_std]
#![deny(warnings, unsafe_code, missing_docs)]
#![allow(clippy::upper_case_acronyms)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use alloc::sync::Arc;
use core::convert::TryFrom;

use kernel_hal::user::{IoVecIn, IoVecOut, UserInOutPtr, UserInPtr, UserOutPtr};
use linux_object::error::{LxError, SysResult};
use linux_object::fs::FileDesc;
use linux_object::process::{LinuxProcess, ProcessExt, RLimit};
use zircon_object::object::{KernelObject, KoID};
use zircon_object::task::{CurrentThread, Process, Thread, ThreadFn};
use zircon_object::vm::VirtAddr;

use self::consts::SyscallType as Sys;

mod consts {
    // generated from syscall.h.in
    include!(concat!(env!("OUT_DIR"), "/consts.rs"));
}

/// Glue for Eclipse's own perf accounting (`/proc/perf`, `/proc/<pid>/perf`).
///
/// Resolves a syscall number to its name via the generated [`Sys`] enum and
/// registers that resolver with `linux-object` (which owns the `/proc` files
/// but not the syscall table) the first time a syscall runs.
mod perf_accounting {
    use super::Sys;
    use alloc::string::{String, ToString};
    use core::convert::TryFrom;
    use core::sync::atomic::{AtomicBool, Ordering};

    fn resolve(num: u32) -> Option<String> {
        // `Sys` is `#[derive(Debug)]`; its variant name is the uppercase
        // syscall name. Lower-case it to match the conventional spelling.
        Sys::try_from(num)
            .ok()
            .map(|s| alloc::format!("{:?}", s).to_lowercase())
            .or_else(|| Some(num.to_string()))
    }

    /// Register the name resolver exactly once.
    pub fn ensure_registered() {
        static DONE: AtomicBool = AtomicBool::new(false);
        if DONE
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            linux_object::perf::set_name_resolver(resolve);
        }
    }
}
/// Max kernel buffer for one read/write/recv/send syscall. Bare-metal zCore uses
/// a fixed kernel heap; large transient allocations (e.g. 1 MiB recv buffers)
/// exhaust or fragment it after long network sessions.
pub(crate) const SYSCALL_IO_MAX: usize = 64 * 1024;

#[cfg(test)]
mod abi;
mod file;
mod ipc;
mod misc;
mod net;
mod signal;
mod task;
mod time;
mod vm;

/// The struct of Syscall which stores the information about making a syscall
pub struct Syscall<'a> {
    /// the thread making a syscall
    pub thread: &'a CurrentThread,
    /// new thread function
    pub thread_fn: ThreadFn,
    /// the entry of current syscall
    pub syscall_entry: VirtAddr,
}

impl Syscall<'_> {
    /// Handle terminal-generated Ctrl+C (SIGINT) in a uniform way.
    ///
    /// This is intentionally centralized so we don't sprinkle per-program hacks.
    fn maybe_handle_tty_intr(&mut self) -> SysResult {
        // Cheap relaxed-style peek first: the latch is empty on the
        // overwhelming majority of syscalls, and a plain load avoids the
        // `lock xchg` on x86 / locked CAS on aarch64/riscv that the swap in
        // `ctrl_c_pending_take` would otherwise issue per syscall.
        if !linux_object::fs::stdio::ctrl_c_pending_peek() {
            return Ok(0);
        }
        // Race-safe: another syscall may have claimed the latch between the
        // peek and the swap.
        if !linux_object::fs::stdio::ctrl_c_pending_take() {
            return Ok(0);
        }
        // Do not use sys_kill(-pgid): pgid==1 becomes kill(-1) ("every process") on Linux.
        linux_object::process::deliver_sigint_to_foreground();
        Err(LxError::EINTR)
    }

    /// syscall entry function
    pub async fn syscall(&mut self, num: u32, args: [usize; 6]) -> isize {
        if let Err(err) = self.maybe_handle_tty_intr() {
            return -(err as isize);
        }
        let pid = self.zircon_process().id();
        if let Err(_err) = hunter::check_syscall(pid, num, &args) {
            return -(linux_object::error::LxError::EPERM as isize);
        }
        let sys_type = Sys::try_from(num);
        debug!(
            "pid: {} syscall: num={} ({:?}), args={:x?}",
            self.zircon_process().id(),
            num,
            sys_type,
            args
        );
        let sys_type = match sys_type {
            Ok(t) => t,
            Err(_) => {
                error!("invalid syscall number: {}", num);
                return LxError::EINVAL as _;
            }
        };
        let [a0, a1, a2, a3, a4, a5] = args;
        // Eclipse's own perf accounting: time every syscall and attribute it to
        // both the system-wide and per-process tables (surfaced at `/proc/perf`
        // and `/proc/<pid>/perf`). The name resolver is registered lazily here
        // so `linux-object` can render numbers as names without an arch table.
        perf_accounting::ensure_registered();
        let perf_start = kernel_hal::timer::timer_now();
        let ret = match sys_type {
            Sys::READ => self.sys_read(a0.into(), a1.into(), a2).await,
            Sys::WRITE => self.sys_write(a0.into(), a1.into(), a2),
            Sys::OPENAT => self.sys_openat(a0.into(), a1.into(), a2, a3),
            Sys::CLOSE => self.sys_close(a0.into()),
            Sys::FSTAT => self.sys_fstat(a0.into(), a1.into()),
            Sys::NEWFSTATAT => self.sys_fstatat(a0.into(), a1.into(), a2.into(), a3),
            Sys::LSEEK => self.sys_lseek(a0.into(), a1 as i64, a2 as u8),
            Sys::IOCTL => self.sys_ioctl(a0.into(), a1, a2, a3, a4),
            Sys::PREAD64 => self.sys_pread(a0.into(), a1.into(), a2, a3 as _).await,
            Sys::PWRITE64 => self.sys_pwrite(a0.into(), a1.into(), a2, a3 as _),
            Sys::READV => self.sys_readv(a0.into(), a1.into(), a2).await,
            Sys::WRITEV => self.sys_writev(a0.into(), a1.into(), a2),
            // Positional vectored I/O. The kernel ABI splits the offset into
            // (pos_l, pos_h) halves; on 64-bit both musl and glibc put the whole
            // offset in pos_l, and the kernel ignores pos_h — so do we.
            Sys::PREADV => self.sys_preadv(a0.into(), a1.into(), a2, a3 as u64).await,
            Sys::PWRITEV => self.sys_pwritev(a0.into(), a1.into(), a2, a3 as u64),
            Sys::PREADV2 => {
                self.sys_preadv2(a0.into(), a1.into(), a2, a3 as i64, a5)
                    .await
            }
            Sys::PWRITEV2 => self.sys_pwritev2(a0.into(), a1.into(), a2, a3 as i64, a5),
            Sys::SENDFILE => self.sys_sendfile(a0.into(), a1.into(), a2.into(), a3).await,
            Sys::FCNTL => self.sys_fcntl(a0.into(), a1, a2),
            Sys::FLOCK => self.sys_flock(a0.into(), a1),
            Sys::FSYNC => self.sys_fsync(a0.into()),
            Sys::FDATASYNC => self.sys_fdatasync(a0.into()),
            Sys::TRUNCATE => self.sys_truncate(a0.into(), a1),
            Sys::FTRUNCATE => self.sys_ftruncate(a0.into(), a1),
            Sys::FADVISE64 => self.sys_fadvise64(a0.into(), a1, a2, a3),
            Sys::FALLOCATE => self.sys_fallocate(a0.into(), a1, a2, a3),
            Sys::READAHEAD => self.sys_readahead(a0.into(), a1 as u64, a2),
            Sys::SYNC_FILE_RANGE => self.sys_sync_file_range(a0.into(), a1 as u64, a2 as u64, a3),
            Sys::GETDENTS64 => self.sys_getdents64(a0.into(), a1.into(), a2),
            Sys::GETCWD => self.sys_getcwd(a0.into(), a1),
            Sys::CHDIR => self.sys_chdir(a0.into()),
            Sys::FCHDIR => self.sys_fchdir(a0.into()),
            Sys::RENAMEAT => self.sys_renameat(a0.into(), a1.into(), a2.into(), a3.into()),
            Sys::MKDIRAT => self.sys_mkdirat(a0.into(), a1.into(), a2),
            Sys::MKNODAT => self.sys_mknodat(a0.into(), a1.into(), a2, a3),
            Sys::LINKAT => self.sys_linkat(a0.into(), a1.into(), a2.into(), a3.into(), a4),
            Sys::UNLINKAT => self.sys_unlinkat(a0.into(), a1.into(), a2),
            Sys::SYMLINKAT => self.sys_symlinkat(a0.into(), a1.into(), a2.into()),
            Sys::READLINKAT => self.sys_readlinkat(a0.into(), a1.into(), a2.into(), a3),
            Sys::FCHMOD => self.sys_fchmod(a0.into(), a1),
            Sys::FCHMODAT => self.sys_fchmodat(a0.into(), a1.into(), a2, a3),
            Sys::FCHOWN => self.sys_fchown(a0.into(), a1, a2),
            Sys::FCHOWNAT => self.sys_fchownat(a0.into(), a1.into(), a2, a3, a4),
            Sys::FACCESSAT => self.sys_faccessat(a0.into(), a1.into(), a2, a3),
            Sys::FACCESSAT2 => self.sys_faccessat(a0.into(), a1.into(), a2, a3),
            Sys::DUP => self.sys_dup(a0.into()),
            Sys::DUP3 => self.sys_dup2(a0.into(), a1.into()), // TODO: handle `flags`
            Sys::PIPE2 => self.sys_pipe2(a0.into(), a1),      // TODO: handle `flags`
            Sys::UTIMENSAT => self.sys_utimensat(a0.into(), a1.into(), a2.into(), a3),
            Sys::COPY_FILE_RANGE => {
                self.sys_copy_file_range(a0.into(), a1.into(), a2.into(), a3.into(), a4, a5)
                    .await
            }
            Sys::SPLICE => {
                self.sys_splice(a0.into(), a1.into(), a2.into(), a3.into(), a4, a5)
                    .await
            }
            Sys::TEE => self.sys_tee(a0.into(), a1.into(), a2, a3).await,
            Sys::VMSPLICE => self.sys_vmsplice(a0.into(), a1, a2, a3).await,
            Sys::CLOSE_RANGE => self.sys_close_range(a0, a1, a2),

            // io multiplexing
            Sys::PSELECT6 => {
                self.sys_pselect6(a0, a1.into(), a2.into(), a3.into(), a4.into(), a5)
                    .await
            }
            Sys::PPOLL => self.sys_ppoll(a0.into(), a1, a2.into()).await, // ignore sigmask
            Sys::EPOLL_CREATE1 => self.sys_epoll_create1(a0),
            Sys::EPOLL_CTL => self.sys_epoll_ctl(a0.into(), a1 as i32, a2.into(), a3.into()),
            Sys::EPOLL_PWAIT => {
                self.sys_epoll_pwait(a0.into(), a1.into(), a2, a3 as isize, a4)
                    .await
            }
            Sys::EVENTFD2 => self.sys_eventfd2(a0 as u32, a1),
            Sys::INOTIFY_INIT => self.sys_inotify_init1(0),
            Sys::INOTIFY_INIT1 => self.sys_inotify_init1(a0),
            Sys::INOTIFY_ADD_WATCH => self.sys_inotify_add_watch(a0, a1.into(), a2 as u32),
            Sys::INOTIFY_RM_WATCH => self.sys_inotify_rm_watch(a0, a1 as i32),
            Sys::MEMFD_CREATE => self.sys_memfd_create(a0.into(), a1),
            Sys::TIMERFD_CREATE => self.sys_timerfd_create(a0, a1),
            Sys::TIMERFD_SETTIME => self.sys_timerfd_settime(a0.into(), a1, a2.into(), a3.into()),
            Sys::TIMERFD_GETTIME => self.sys_timerfd_gettime(a0.into(), a1.into()),
            Sys::SIGNALFD4 => self.sys_signalfd4(a0.into(), a1.into(), a2, a3),
            Sys::SIGNALFD => self.sys_signalfd4(a0.into(), a1.into(), a2, 0),

            Sys::SOCKETPAIR => self.sys_socketpair(a0, a1, a2, a3.into()),
            // file system
            Sys::STATFS => self.sys_statfs(a0.into(), a1.into()),
            Sys::FSTATFS => self.sys_fstatfs(a0.into(), a1.into()),
            Sys::SYNC => self.sys_sync(),
            Sys::MOUNT => self.sys_mount(a0.into(), a1.into(), a2.into(), a3, a4.into()),
            Sys::UMOUNT2 => self.sys_umount2(a0.into(), a1),

            // memory
            Sys::BRK => self.sys_brk(a0),
            Sys::MMAP => self.sys_mmap(a0, a1, a2, a3, a4.into(), a5 as _).await,
            Sys::MPROTECT => self.sys_mprotect(a0, a1, a2),
            Sys::MUNMAP => self.sys_munmap(a0, a1),
            Sys::MADVISE => self.sys_madvise(a0, a1, a2),
            Sys::MREMAP => self.sys_mremap(a0, a1, a2, a3, a4),
            Sys::MSYNC => self.sys_msync(a0, a1, a2),
            Sys::MINCORE => self.sys_mincore(a0, a1, a2.into()),
            Sys::MBIND => self.unimplemented("mbind", Err(LxError::ENOSYS)),
            Sys::GET_MEMPOLICY => self.unimplemented("get_mempolicy", Err(LxError::ENOSYS)),
            Sys::SET_MEMPOLICY => self.unimplemented("set_mempolicy", Err(LxError::ENOSYS)),

            // signal
            Sys::RT_SIGACTION => self.sys_rt_sigaction(a0, a1.into(), a2.into(), a3),
            Sys::RT_SIGPROCMASK => self.sys_rt_sigprocmask(a0 as _, a1.into(), a2.into(), a3),
            Sys::RT_SIGRETURN => self.sys_rt_sigreturn(),
            Sys::RT_SIGSUSPEND => self.sys_rt_sigsuspend(a0.into(), a1).await,
            Sys::RT_SIGTIMEDWAIT => {
                self.sys_rt_sigtimedwait(a0.into(), a1.into(), a2.into(), a3)
                    .await
            }
            Sys::SIGALTSTACK => self.sys_sigaltstack(a0.into(), a1.into()),
            Sys::KILL => self.sys_kill(a0 as isize, a1),

            // schedule
            Sys::SCHED_YIELD => {
                kernel_hal::thread::yield_now().await;
                Ok(0)
            }
            Sys::SCHED_GETAFFINITY => self.sys_sched_getaffinity(a0, a1, a2.into()),
            Sys::SCHED_SETAFFINITY => self.sys_sched_setaffinity(a0, a1, a2.into()),
            Sys::SCHED_SETSCHEDULER => self.sys_sched_setscheduler(a0, a1, a2.into()),
            Sys::SCHED_GETSCHEDULER => self.sys_sched_getscheduler(a0),
            Sys::SCHED_SETPARAM => self.sys_sched_setparam(a0, a1.into()),
            Sys::SCHED_GETPARAM => self.sys_sched_getparam(a0, a1.into()),
            Sys::SCHED_GET_PRIORITY_MAX => self.sys_sched_get_priority_max(a0),
            Sys::SCHED_GET_PRIORITY_MIN => self.sys_sched_get_priority_min(a0),
            Sys::SCHED_RR_GET_INTERVAL => self.sys_sched_rr_get_interval(a0, a1.into()),
            Sys::SCHED_SETATTR => self.sys_sched_setattr(a0, a1.into(), a2),
            Sys::SCHED_GETATTR => self.sys_sched_getattr(a0, a1.into(), a2, a3),

            // socket
            Sys::SOCKET => self.sys_socket(a0, a1, a2),
            Sys::CONNECT => self.sys_connect(a0, a1.into(), a2).await,
            Sys::ACCEPT => self.sys_accept(a0, a1.into(), a2.into()).await,
            // accept4 == accept + flags on the NEW socket (SOCK_CLOEXEC /
            // SOCK_NONBLOCK). GLib/GDBus uses it unconditionally; falling
            // through to `unknown syscall` broke waybar's D-Bus socket path.
            Sys::ACCEPT4 => self.sys_accept4(a0, a1.into(), a2.into(), a3).await,
            Sys::SENDTO => self.sys_sendto(a0, a1.into(), a2, a3, a4.into(), a5),
            Sys::RECVFROM => {
                self.sys_recvfrom(a0, a1.into(), a2, a3, a4.into(), a5.into())
                    .await
            }
            Sys::SENDMSG => self.sys_sendmsg(a0, a1.into(), a2),
            Sys::RECVMSG => self.sys_recvmsg(a0, a1.into(), a2).await,
            Sys::SENDMMSG => self.sys_sendmmsg(a0, a1, a2, a3),
            Sys::RECVMMSG => self.sys_recvmmsg(a0, a1, a2, a3, a4).await,
            Sys::SHUTDOWN => self.sys_shutdown(a0, a1),
            Sys::BIND => self.sys_bind(a0, a1.into(), a2),
            Sys::LISTEN => self.sys_listen(a0, a1),

            Sys::GETSOCKNAME => self.sys_getsockname(a0, a1.into(), a2.into()),
            Sys::GETPEERNAME => self.sys_getpeername(a0, a1.into(), a2.into()),
            Sys::SETSOCKOPT => self.sys_setsockopt(a0, a1, a2, a3.into(), a4),
            Sys::GETSOCKOPT => self.sys_getsockopt(a0, a1, a2, a3.into(), a4.into()),

            // process
            Sys::EXECVE => self.sys_execve(a0.into(), a1.into(), a2.into()),
            // clone3 is deliberately ENOSYS (pre-Linux-5.3 behaviour; glibc and
            // musl fall back to legacy clone cleanly). Root cause, found in the
            // QEMU desktop lab: glibc's __clone3 child stub starts with
            // `mov %r8,%rdi; call *%rdx` — it requires RDX (and R8) to survive
            // the syscall INTO THE NEW CHILD. Legacy clone's stub instead pops
            // the function/argument off the child STACK, which is robust. Our
            // new-thread first-entry path does not preserve the parent's RDX
            // into the child, so clone3-started threads jumped to garbage
            // (observed: kernel wild jump to 0x400000006, a #GP on a
            // non-canonical pointer inside epoll_pwait, and an all-idle wedge
            // when the crashed thread held a compositor lock). Until the child
            // context provably carries every caller-saved register, answering
            // ENOSYS is the correct, safe behaviour — sys_clone3 below stays
            // implemented for when that is fixed.
            Sys::CLONE3 => Err(LxError::ENOSYS),
            #[allow(unreachable_patterns)]
            Sys::CLONE3 => self.sys_clone3(a0.into(), a1).await,
            Sys::EXIT => self.sys_exit(a0 as _),
            Sys::EXIT_GROUP => self.sys_exit_group(a0 as _),
            Sys::WAIT4 => self.sys_wait4(a0 as _, a1.into(), a2 as _, a3.into()).await,
            Sys::WAITID => self.sys_waitid(a0 as i32, a1, a2.into(), a3 as u32).await,
            Sys::SET_TID_ADDRESS => self.sys_set_tid_address(a0.into()),
            Sys::FUTEX => self.sys_futex(a0, a1 as _, a2 as _, a3, a4, a5 as _).await,
            Sys::GET_ROBUST_LIST => self.sys_get_robust_list(a0 as _, a1.into(), a2.into()),
            Sys::SET_ROBUST_LIST => self.sys_set_robust_list(a0.into(), a1 as _),
            Sys::TKILL => self.sys_tkill(a0, a1),
            Sys::TGKILL => self.sys_tgkill(a0, a1, a2),
            Sys::PIDFD_OPEN => self.sys_pidfd_open(a0, a1 as u32),
            Sys::PIDFD_SEND_SIGNAL => {
                self.sys_pidfd_send_signal(a0.into(), a1, a2.into(), a3 as u32)
            }
            Sys::PIDFD_GETFD => self.sys_pidfd_getfd(a0.into(), a1 as i32, a2 as u32),

            // time
            Sys::NANOSLEEP => self.sys_nanosleep(a0.into()).await,
            Sys::CLOCK_NANOSLEEP => self.sys_clock_nanosleep(a0, a1, a2.into(), a3.into()).await,
            Sys::SETITIMER => self.sys_setitimer(a0, a1.into(), a2.into()),
            // `alarm` only exists in the x86_64 syscall table; the generic ABI
            // (aarch64/riscv64) omits it in favour of setitimer/timer_*.
            #[cfg(target_arch = "x86_64")]
            Sys::ALARM => self.sys_alarm(a0),
            Sys::TIMER_CREATE => self.sys_timer_create(a0, a1, a2),
            Sys::TIMER_SETTIME => self.sys_timer_settime(a0, a1, a2.into(), a3.into()),
            Sys::TIMER_GETTIME => self.sys_timer_gettime(a0, a1),
            Sys::TIMER_DELETE => self.sys_timer_delete(a0),
            Sys::TIMER_GETOVERRUN => self.sys_timer_getoverrun(a0),
            Sys::GETITIMER => self.sys_getitimer(a0, a1.into()),
            Sys::GETTIMEOFDAY => self.sys_gettimeofday(a0.into(), a1.into()),
            Sys::SETTIMEOFDAY => self.sys_settimeofday(a0.into(), a1.into()),
            Sys::CLOCK_GETTIME => self.sys_clock_gettime(a0, a1.into()),
            Sys::CLOCK_SETTIME => self.sys_clock_settime(a0, a1.into()),
            Sys::CLOCK_GETRES => self.sys_clock_getres(a0, a1.into()),

            // sem
            #[cfg(not(target_arch = "mips"))]
            Sys::SEMGET => self.sys_semget(a0, a1, a2),
            #[cfg(not(target_arch = "mips"))]
            Sys::SEMOP => self.sys_semop(a0, a1.into(), a2).await,
            #[cfg(not(target_arch = "mips"))]
            Sys::SEMCTL => self.sys_semctl(a0, a1, a2, a3),

            // shm
            #[cfg(not(target_arch = "mips"))]
            Sys::SHMGET => self.sys_shmget(a0, a1, a2),
            #[cfg(not(target_arch = "mips"))]
            Sys::SHMAT => self.sys_shmat(a0, a1, a2),
            #[cfg(not(target_arch = "mips"))]
            Sys::SHMDT => self.sys_shmdt(a0, a1, a2),
            #[cfg(not(target_arch = "mips"))]
            Sys::SHMCTL => self.sys_shmctl(a0, a1, a2),

            // system
            Sys::GETPID => self.sys_getpid(),
            Sys::GETTID => self.sys_gettid(),
            Sys::GETCPU => self.sys_getcpu(a0.into(), a1.into(), a2),
            Sys::UNAME => self.sys_uname(a0.into()),
            Sys::SETHOSTNAME => self.sys_sethostname(a0.into(), a1),
            Sys::SETDOMAINNAME => self.sys_setdomainname(a0.into(), a1),
            Sys::CAPGET => self.sys_capget(a0.into(), a1.into()),
            Sys::CAPSET => self.sys_capset(a0.into(), a1.into()),
            Sys::IOPRIO_SET => self.sys_ioprio_set(a0, a1, a2),
            Sys::IOPRIO_GET => self.sys_ioprio_get(a0, a1),
            Sys::SYSLOG => self.sys_syslog(a0 as i32, a1.into(), a2 as i32),
            Sys::UMASK => self.sys_umask(a0),
            Sys::GETRLIMIT => self.sys_getrlimit(a0, a1.into()),
            Sys::SETRLIMIT => self.sys_setrlimit(a0, a1.into()),
            Sys::GETRUSAGE => self.sys_getrusage(a0, a1.into()),
            Sys::SYSINFO => self.sys_sysinfo(a0.into()),
            Sys::TIMES => self.sys_times(a0.into()),
            Sys::GETUID => self.sys_getuid(),
            Sys::GETGID => self.sys_getgid(),
            Sys::SETUID => self.sys_setuid(a0),
            Sys::SETGID => self.sys_setgid(a0),
            Sys::SETREUID => self.sys_setreuid(a0, a1),
            Sys::SETREGID => self.sys_setregid(a0, a1),
            Sys::SETRESUID => self.sys_setresuid(a0, a1, a2),
            Sys::SETRESGID => self.sys_setresgid(a0, a1, a2),
            Sys::GETRESUID => self.sys_getresuid(a0.into(), a1.into(), a2.into()),
            Sys::GETRESGID => self.sys_getresgid(a0.into(), a1.into(), a2.into()),
            Sys::SETFSUID => self.sys_setfsuid(a0),
            Sys::SETFSGID => self.sys_setfsgid(a0),
            Sys::GETEUID => self.sys_geteuid(),
            Sys::GETEGID => self.sys_getegid(),
            Sys::SETPGID => self.sys_setpgid(a0, a1),
            Sys::GETPPID => self.sys_getppid(),
            Sys::SETSID => self.sys_setsid(),
            Sys::GETPGID => self.sys_getpgid(a0),
            // getpgrp() is the legacy no-argument form of getpgid(0). Without it
            // an interactive busybox `sh` cannot determine its own process group
            // during job-control setup, takes the "I am a background job" branch
            // and `kill(0, SIGTTIN)`s itself — which then terminated the shell.
            Sys::GETPGRP => self.sys_getpgid(0),
            Sys::GETGROUPS => self.sys_getgroups(a0, a1.into()),
            Sys::SETGROUPS => self.sys_setgroups(a0, a1.into()),
            // Scheduling priority (nice). Backed by the thread's stored nice
            // value, which also biases its timeslice (see
            // `Thread::tick_should_preempt`). getpriority returns `20 - nice`
            // so that valid values stay non-negative.
            Sys::SETPRIORITY => self.sys_setpriority(a0, a1, a2 as i32),
            Sys::GETPRIORITY => self.sys_getpriority(a0, a1),
            // prctl: accept-and-ignore. glibc issues several prctls per thread
            // start (PR_SET_NAME, etc.); returning 0 quietly keeps exec-heavy
            // workloads from flooding the log (a `warn!` per call was very loud
            // under `perf`). Trace-level only for when it is actually needed.
            Sys::PRCTL => {
                trace!("prctl(a0={:#x}) ignored -> 0", a0);
                Ok(0)
            }
            // `rseq` (restartable sequences) is optional: glibc probes it on
            // every thread start and silently falls back when it is missing.
            // Return ENOSYS quietly so we don't (a) advertise a feature we don't
            // implement, nor (b) flood the log with "unknown syscall: RSEQ" on
            // every process spawn (very visible under `perf`/exec-heavy loads).
            Sys::RSEQ => Err(LxError::ENOSYS),
            // Same treatment: glibc/util-linux/wlroots stacks probe
            // name_to_handle_at on hotplug/device paths and handle ENOSYS
            // fine; the loud per-call ERROR line was pure log noise.
            Sys::NAME_TO_HANDLE_AT => Err(LxError::ENOSYS),
            Sys::MEMBARRIER => self.sys_membarrier(a0 as i32, a1 as u32, a2 as i32),
            Sys::PRLIMIT64 => self.sys_prlimit64(a0, a1, a2.into(), a3.into()),
            Sys::REBOOT => self.sys_reboot(a0 as u32, a1 as u32, a2 as u32, a3.into()),
            Sys::GETRANDOM => self.sys_getrandom(a0.into(), a1 as usize, a2 as u32),
            Sys::STATX => self.sys_statx(a0.into(), a1.into(), a2, a3 as u32, a4.into()),
            Sys::RT_SIGQUEUEINFO => self.unimplemented("rt_sigqueueinfo", Ok(0)),

            // Extended attributes: this kernel's filesystems do not implement
            // xattrs. Answer the standard "no xattr support" way and quietly —
            // letting these fall through to `unknown_syscall` returned ENOSYS
            // but logged an `error!` per call, which floods the console (e.g.
            // busybox init probing files: `unknown syscall: LISTXATTR`).
            // `listxattr` -> 0 (empty name list); `getxattr` -> ENODATA (no such
            // attribute); `setxattr` -> EOPNOTSUPP; `removexattr` -> ENODATA.
            Sys::LISTXATTR | Sys::LLISTXATTR | Sys::FLISTXATTR => Ok(0),
            Sys::GETXATTR | Sys::LGETXATTR | Sys::FGETXATTR => Err(LxError::ENODATA),
            Sys::SETXATTR | Sys::LSETXATTR | Sys::FSETXATTR => Err(LxError::EOPNOTSUPP),
            Sys::REMOVEXATTR | Sys::LREMOVEXATTR | Sys::FREMOVEXATTR => Err(LxError::ENODATA),

            // kernel module
            //            Sys::INIT_MODULE => self.sys_init_module(a0.into(), a1 as usize, a2.into()),
            Sys::FINIT_MODULE => self.unimplemented("finit_module", Err(LxError::ENOSYS)),
            //            Sys::DELETE_MODULE => self.sys_delete_module(a0.into(), a1 as u32),
            #[cfg(not(target_arch = "aarch64"))]
            Sys::BLOCK_IN_KERNEL => self.sys_block_in_kernel(),
            Sys::ECLIPSE_DNS_QUERY => self.sys_eclipse_dns_query(a0.into(), a1, a2, a3.into(), a4),
            Sys::PERF_EVENT_OPEN => {
                self.sys_perf_event_open(a0, a1 as i32, a2 as i32, a3 as i32, a4)
            }

            #[cfg(target_arch = "x86_64")]
            _ => self.x86_64_syscall(sys_type, args).await,
            #[cfg(target_arch = "riscv64")]
            _ => self.riscv64_syscall(sys_type, args).await,
            #[cfg(target_arch = "aarch64")]
            _ => self.aarch64_syscall(sys_type, args).await,
        };
        // `checked_sub` (not `-`): an async syscall can migrate CPUs across an
        // await, and with unsynchronised TSCs the end can read before the start,
        // which would panic on a plain `Duration` subtraction.
        let elapsed_ns = kernel_hal::timer::timer_now()
            .checked_sub(perf_start)
            .unwrap_or_default()
            .as_nanos() as u64;
        linux_object::perf::record(self.linux_process(), num, elapsed_ns);
        info!("<= {:?}", ret);
        match ret {
            Ok(value) => value as isize,
            Err(err) => -(err as isize),
        }
    }

    #[cfg(target_arch = "aarch64")]
    /// syscall specified for aarch64
    async fn aarch64_syscall(&mut self, sys_type: Sys, args: [usize; 6]) -> SysResult {
        let [a0, a1, a2, a3, a4, _a5] = args;
        debug!("aarch6464_syscall: {:?}, args: {:?}", sys_type, args);
        match sys_type {
            Sys::CLONE => self.sys_clone(a0, a1, a2.into(), a3, a4.into()).await,
            _ => self.unknown_syscall(sys_type),
        }
    }

    #[cfg(target_arch = "x86_64")]
    /// syscall specified for x86_64
    async fn x86_64_syscall(&mut self, sys_type: Sys, args: [usize; 6]) -> SysResult {
        let [a0, a1, a2, a3, a4, _a5] = args;
        match sys_type {
            Sys::OPEN => self.sys_open(a0.into(), a1, a2),
            Sys::STAT => self.sys_stat(a0.into(), a1.into()),
            Sys::LSTAT => self.sys_lstat(a0.into(), a1.into()),
            Sys::POLL => self.sys_poll(a0.into(), a1, a2 as _).await,
            Sys::ACCESS => self.sys_access(a0.into(), a1),
            Sys::PIPE => self.sys_pipe(a0.into()),
            Sys::SELECT => {
                self.sys_select(a0, a1.into(), a2.into(), a3.into(), a4.into())
                    .await
            }
            Sys::DUP2 => self.sys_dup2(a0.into(), a1.into()),
            //            Sys::ALARM => self.unimplemented("alarm", Ok(0)),
            Sys::PAUSE => self.sys_pause().await,
            Sys::FORK => self.sys_fork(0, 0),
            Sys::VFORK => self.sys_vfork(0, 0).await,
            Sys::RENAME => self.sys_rename(a0.into(), a1.into()),
            Sys::MKDIR => self.sys_mkdir(a0.into(), a1),
            Sys::MKNOD => self.sys_mknod(a0.into(), a1, a2),
            Sys::RMDIR => self.sys_rmdir(a0.into()),
            Sys::LINK => self.sys_link(a0.into(), a1.into()),
            Sys::UNLINK => self.sys_unlink(a0.into()),
            Sys::SYMLINK => self.sys_symlink(a0.into(), a1.into()),
            Sys::READLINK => self.sys_readlink(a0.into(), a1.into(), a2),
            Sys::CHMOD => self.sys_chmod(a0.into(), a1),
            Sys::CHOWN => self.sys_fchownat(FileDesc::CWD, a0.into(), a1, a2, 0),
            Sys::ARCH_PRCTL => self.sys_arch_prctl(a0 as _, a1),
            Sys::TIME => self.sys_time(a0.into()),
            Sys::CLONE => self.sys_clone(a0, a1, a2.into(), a4, a3.into()).await,
            Sys::EPOLL_CREATE => self.sys_epoll_create(a0),
            Sys::EPOLL_WAIT => {
                self.sys_epoll_wait(a0.into(), a1.into(), a2, a3 as isize)
                    .await
            }
            _ => self.unknown_syscall(sys_type),
        }
    }

    #[cfg(target_arch = "riscv64")]
    async fn riscv64_syscall(&mut self, sys_type: Sys, args: [usize; 6]) -> SysResult {
        let [a0, a1, a2, a3, a4, _a5] = args;
        match sys_type {
            //Sys::OPEN => self.sys_open(a0.into(), a1, a2),
            Sys::CLONE => self.sys_clone(a0, a1, a2.into(), a3, a4.into()).await,
            _ => self.unknown_syscall(sys_type),
        }
    }

    /// unkown syscalls, currently is similar to unimplemented syscalls but emit an error
    fn unknown_syscall(&mut self, sys_type: Sys) -> SysResult {
        error!("unknown syscall: {:?}.", sys_type);
        Err(LxError::ENOSYS)
    }

    /// unimplemented syscalls
    fn unimplemented(&self, name: &str, ret: SysResult) -> SysResult {
        warn!("{}: unimplemented", name);
        ret
    }

    /// get zircon process
    fn zircon_process(&self) -> &Arc<Process> {
        self.thread.proc()
    }

    /// get linux process
    fn linux_process(&self) -> &LinuxProcess {
        self.zircon_process().linux()
    }
}
