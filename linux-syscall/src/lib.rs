//! Linux syscall implementations

#![no_std]
#![deny(warnings, unsafe_code, unused_must_use, unreachable_patterns)]
#![feature(bool_to_option)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use {
    self::consts::SyscallType as Sys,
    alloc::sync::Arc,
    core::convert::TryFrom,
    kernel_hal::{user::*, UserContext},
    linux_object::{error::*, fs::FileDesc, process::*},
    zircon_object::{object::*, task::*, vm::VirtAddr},
};

mod consts;
mod file;
mod misc;
mod task;
mod time;
mod vm;

pub struct Syscall<'a> {
    pub thread: &'a Arc<Thread>,
    pub syscall_entry: VirtAddr,
    pub context: &'a mut UserContext,
    pub spawn_fn: fn(thread: Arc<Thread>),
    /// Set `true` to exit current task.
    pub exit: bool,
}

impl Syscall<'_> {
    pub async fn syscall(&mut self, num: u32, args: [usize; 6]) -> isize {
        debug!("syscall: num={}, args={:x?}", num, args);
        let sys_type = match Sys::try_from(num) {
            Ok(t) => t,
            Err(_) => {
                error!("invalid syscall number: {}", num);
                return LxError::EINVAL as _;
            }
        };
        let [a0, a1, a2, a3, a4, a5] = args;
        let ret = match sys_type {
            Sys::READ => self.sys_read(a0.into(), a1.into(), a2),
            Sys::WRITE => self.sys_write(a0.into(), a1.into(), a2),
            Sys::OPENAT => self.sys_openat(a0.into(), a1.into(), a2, a3),
            Sys::CLOSE => self.sys_close(a0.into()),
            Sys::FSTAT => self.sys_fstat(a0.into(), a1.into()),
            Sys::NEWFSTATAT => self.sys_fstatat(a0.into(), a1.into(), a2.into(), a3),
            Sys::LSEEK => self.sys_lseek(a0.into(), a1 as i64, a2 as u8),
            Sys::IOCTL => self.sys_ioctl(a0.into(), a1, a2, a3, a4),
            Sys::PREAD64 => self.sys_pread(a0.into(), a1.into(), a2, a3 as _),
            Sys::PWRITE64 => self.sys_pwrite(a0.into(), a1.into(), a2, a3 as _),
            Sys::READV => self.sys_readv(a0.into(), a1.into(), a2),
            Sys::WRITEV => self.sys_writev(a0.into(), a1.into(), a2),
            Sys::SENDFILE => self.sys_sendfile(a0.into(), a1.into(), a2.into(), a3),
            Sys::FCNTL => self.sys_fcntl(a0.into(), a1, a2),
            Sys::FLOCK => self.unimplemented("flock", Ok(0)),
            Sys::FSYNC => self.sys_fsync(a0.into()),
            Sys::FDATASYNC => self.sys_fdatasync(a0.into()),
            Sys::TRUNCATE => self.sys_truncate(a0.into(), a1),
            Sys::FTRUNCATE => self.sys_ftruncate(a0.into(), a1),
            Sys::GETDENTS64 => self.sys_getdents64(a0.into(), a1.into(), a2),
            Sys::GETCWD => self.sys_getcwd(a0.into(), a1),
            Sys::CHDIR => self.sys_chdir(a0.into()),
            Sys::RENAMEAT => self.sys_renameat(a0.into(), a1.into(), a2.into(), a3.into()),
            Sys::MKDIRAT => self.sys_mkdirat(a0.into(), a1.into(), a2),
            Sys::LINKAT => self.sys_linkat(a0.into(), a1.into(), a2.into(), a3.into(), a4),
            Sys::UNLINKAT => self.sys_unlinkat(a0.into(), a1.into(), a2),
            Sys::SYMLINKAT => self.unimplemented("symlinkat", Err(LxError::EACCES)),
            Sys::READLINKAT => self.sys_readlinkat(a0.into(), a1.into(), a2.into(), a3),
            Sys::FCHMOD => self.unimplemented("fchmod", Ok(0)),
            Sys::FCHMODAT => self.unimplemented("fchmodat", Ok(0)),
            Sys::FCHOWN => self.unimplemented("fchown", Ok(0)),
            Sys::FCHOWNAT => self.unimplemented("fchownat", Ok(0)),
            Sys::FACCESSAT => self.sys_faccessat(a0.into(), a1.into(), a2, a3),
            Sys::DUP3 => self.sys_dup2(a0.into(), a1.into()), // TODO: handle `flags`
            //            Sys::PIPE2 => self.sys_pipe(a0.into()),           // TODO: handle `flags`
            Sys::UTIMENSAT => self.unimplemented("utimensat", Ok(0)),
            Sys::COPY_FILE_RANGE => {
                self.sys_copy_file_range(a0.into(), a1.into(), a2.into(), a3.into(), a4, a5)
            }

            // io multiplexing
            //            Sys::PSELECT6 => self.sys_pselect6(a0, a1.into(), a2.into(), a3.into(), a4.into(), a5.into()),
            //            Sys::PPOLL => self.sys_ppoll(a0.into(), a1, a2.into()), // ignore sigmask
            //            Sys::EPOLL_CREATE1 => self.sys_epoll_create1(a0),
            //            Sys::EPOLL_CTL => self.sys_epoll_ctl(a0, a1, a2, a3.into()),
            //            Sys::EPOLL_PWAIT => self.sys_epoll_pwait(a0, a1.into(), a2, a3, a4),
            //            Sys::EVENTFD2 => self.unimplemented("eventfd2", Err(LxError::EACCES)),

            //            Sys::SOCKETPAIR => self.unimplemented("socketpair", Err(LxError::EACCES)),
            // file system
            Sys::STATFS => self.unimplemented("statfs", Err(LxError::EACCES)),
            Sys::FSTATFS => self.unimplemented("fstatfs", Err(LxError::EACCES)),
            Sys::SYNC => self.sys_sync(),
            Sys::MOUNT => self.unimplemented("mount", Err(LxError::EACCES)),
            Sys::UMOUNT2 => self.unimplemented("umount2", Err(LxError::EACCES)),

            // memory
            Sys::BRK => self.unimplemented("brk", Err(LxError::ENOMEM)),
            Sys::MMAP => self.sys_mmap(a0, a1, a2, a3, a4.into(), a5 as _),
            Sys::MPROTECT => self.sys_mprotect(a0, a1, a2),
            Sys::MUNMAP => self.sys_munmap(a0, a1),
            Sys::MADVISE => self.unimplemented("madvise", Ok(0)),

            // signal
            Sys::RT_SIGACTION => self.unimplemented("sigaction", Ok(0)),
            Sys::RT_SIGPROCMASK => self.unimplemented("sigprocmask", Ok(0)),
            Sys::SIGALTSTACK => self.unimplemented("sigaltstack", Ok(0)),
            //            Sys::KILL => self.sys_kill(a0, a1),

            // schedule
            //            Sys::SCHED_YIELD => self.sys_yield(),
            //            Sys::SCHED_GETAFFINITY => self.sys_sched_getaffinity(a0, a1, a2.into()),

            // socket
            //            Sys::SOCKET => self.sys_socket(a0, a1, a2),
            //            Sys::CONNECT => self.sys_connect(a0, a1.into(), a2),
            //            Sys::ACCEPT => self.sys_accept(a0, a1.into(), a2.into()),
            //            Sys::ACCEPT4 => self.sys_accept(a0, a1.into(), a2.into()), // use accept for accept4
            //            Sys::SENDTO => self.sys_sendto(a0, a1.into(), a2, a3, a4.into(), a5),
            //            Sys::RECVFROM => self.sys_recvfrom(a0, a1.into(), a2, a3, a4.into(), a5.into()),
            //            Sys::SENDMSG => self.sys_sendmsg(),
            //            Sys::RECVMSG => self.sys_recvmsg(a0, a1.into(), a2),
            //            Sys::SHUTDOWN => self.sys_shutdown(a0, a1),
            //            Sys::BIND => self.sys_bind(a0, a1.into(), a2),
            //            Sys::LISTEN => self.sys_listen(a0, a1),
            //            Sys::GETSOCKNAME => self.sys_getsockname(a0, a1.into(), a2.into()),
            //            Sys::GETPEERNAME => self.sys_getpeername(a0, a1.into(), a2.into()),
            //            Sys::SETSOCKOPT => self.sys_setsockopt(a0, a1, a2, a3.into(), a4),
            //            Sys::GETSOCKOPT => self.sys_getsockopt(a0, a1, a2, a3.into(), a4.into()),

            // process
            Sys::CLONE => self.sys_clone(a0, a1, a2.into(), a3.into(), a4),
            Sys::EXECVE => self.sys_execve(a0.into(), a1.into(), a2.into()),
            Sys::EXIT => self.sys_exit(a0 as _),
            Sys::EXIT_GROUP => self.sys_exit_group(a0 as _),
            Sys::WAIT4 => self.sys_wait4(a0 as _, a1.into(), a2 as _).await,
            Sys::SET_TID_ADDRESS => self.sys_set_tid_address(a0.into()),
            Sys::FUTEX => self.sys_futex(a0, a1 as _, a2 as _, a3.into()).await,
            Sys::TKILL => self.unimplemented("tkill", Ok(0)),

            // time
            //            Sys::NANOSLEEP => self.sys_nanosleep(a0.into()),
            Sys::SETITIMER => self.unimplemented("setitimer", Ok(0)),
            //            Sys::GETTIMEOFDAY => self.sys_gettimeofday(a0.into(), a1.into()),
            Sys::CLOCK_GETTIME => self.sys_clock_gettime(a0, a1.into()),

            // sem
            //            #[cfg(not(target_arch = "mips"))]
            //            Sys::SEMGET => self.sys_semget(a0, a1 as isize, a2),
            //            #[cfg(not(target_arch = "mips"))]
            //            Sys::SEMOP => self.sys_semop(a0, a1.into(), a2),
            //            #[cfg(not(target_arch = "mips"))]
            //            Sys::SEMCTL => self.sys_semctl(a0, a1, a2, a3 as isize),

            // system
            Sys::GETPID => self.sys_getpid(),
            Sys::GETTID => self.sys_gettid(),
            Sys::UNAME => self.sys_uname(a0.into()),
            Sys::UMASK => self.unimplemented("umask", Ok(0o777)),
            //            Sys::GETRLIMIT => self.sys_getrlimit(),
            //            Sys::SETRLIMIT => self.sys_setrlimit(),
            //            Sys::GETRUSAGE => self.sys_getrusage(a0, a1.into()),
            //            Sys::SYSINFO => self.sys_sysinfo(a0.into()),
            //            Sys::TIMES => self.sys_times(a0.into()),
            Sys::GETUID => self.unimplemented("getuid", Ok(0)),
            Sys::GETGID => self.unimplemented("getgid", Ok(0)),
            Sys::SETUID => self.unimplemented("setuid", Ok(0)),
            Sys::GETEUID => self.unimplemented("geteuid", Ok(0)),
            Sys::GETEGID => self.unimplemented("getegid", Ok(0)),
            Sys::SETPGID => self.unimplemented("setpgid", Ok(0)),
            Sys::GETPPID => self.sys_getppid(),
            Sys::SETSID => self.unimplemented("setsid", Ok(0)),
            Sys::GETPGID => self.unimplemented("getpgid", Ok(0)),
            Sys::GETGROUPS => self.unimplemented("getgroups", Ok(0)),
            Sys::SETGROUPS => self.unimplemented("setgroups", Ok(0)),
            //            Sys::SETPRIORITY => self.sys_set_priority(a0),
            Sys::PRCTL => self.unimplemented("prctl", Ok(0)),
            Sys::MEMBARRIER => self.unimplemented("membarrier", Ok(0)),
            //            Sys::PRLIMIT64 => self.sys_prlimit64(a0, a1, a2.into(), a3.into()),
            //            Sys::REBOOT => self.sys_reboot(a0 as u32, a1 as u32, a2 as u32, a3.into()),
            //            Sys::GETRANDOM => self.sys_getrandom(a0.into(), a1 as usize, a2 as u32),
            Sys::RT_SIGQUEUEINFO => self.unimplemented("rt_sigqueueinfo", Ok(0)),

            // kernel module
            //            Sys::INIT_MODULE => self.sys_init_module(a0.into(), a1 as usize, a2.into()),
            Sys::FINIT_MODULE => self.unimplemented("finit_module", Err(LxError::ENOSYS)),
            //            Sys::DELETE_MODULE => self.sys_delete_module(a0.into(), a1 as u32),
            #[cfg(target_arch = "x86_64")]
            _ => self.x86_64_syscall(sys_type, args).await,
            #[cfg(target_arch = "mips")]
            _ => self.mips_syscall(sys_type, args).await,
        };
        info!("<= {:x?}", ret);
        match ret {
            Ok(value) => value as isize,
            Err(err) => -(err as isize),
        }
    }

    #[cfg(target_arch = "mips")]
    async fn mips_syscall(&mut self, _sys_type: Sys, _args: [usize; 6]) -> SysResult {
        // self.unknown_syscall(sys_type);
        unimplemented!();
    }

    #[cfg(target_arch = "x86_64")]
    async fn x86_64_syscall(&mut self, sys_type: Sys, args: [usize; 6]) -> SysResult {
        let [a0, a1, a2, _a3, _a4, _a5] = args;
        match sys_type {
            Sys::OPEN => self.sys_open(a0.into(), a1, a2),
            Sys::STAT => self.sys_stat(a0.into(), a1.into()),
            Sys::LSTAT => self.sys_lstat(a0.into(), a1.into()),
            //            Sys::POLL => self.sys_poll(a0.into(), a1, a2),
            Sys::ACCESS => self.sys_access(a0.into(), a1),
            //            Sys::PIPE => self.sys_pipe(a0.into()),
            //            Sys::SELECT => self.sys_select(a0, a1.into(), a2.into(), a3.into(), a4.into()),
            Sys::DUP2 => self.sys_dup2(a0.into(), a1.into()),
            //            Sys::ALARM => self.unimplemented("alarm", Ok(0)),
            Sys::FORK => self.sys_fork(),
            Sys::VFORK => self.sys_vfork().await,
            Sys::RENAME => self.sys_rename(a0.into(), a1.into()),
            Sys::MKDIR => self.sys_mkdir(a0.into(), a1),
            Sys::RMDIR => self.sys_rmdir(a0.into()),
            Sys::LINK => self.sys_link(a0.into(), a1.into()),
            Sys::UNLINK => self.sys_unlink(a0.into()),
            Sys::READLINK => self.sys_readlink(a0.into(), a1.into(), a2),
            //            Sys::CHMOD => self.unimplemented("chmod", Ok(0)),
            //            Sys::CHOWN => self.unimplemented("chown", Ok(0)),
            Sys::ARCH_PRCTL => self.sys_arch_prctl(a0 as _, a1),
            //            Sys::TIME => self.sys_time(a0 as *mut u64),
            //            Sys::EPOLL_CREATE => self.sys_epoll_create(a0),
            //            Sys::EPOLL_WAIT => self.sys_epoll_wait(a0, a1.into(), a2, a3),
            _ => self.unknown_syscall(sys_type),
        }
    }

    #[allow(dead_code)]
    fn unknown_syscall(&mut self, sys_type: Sys) -> SysResult {
        error!("unknown syscall: {:?}. exit...", sys_type);
        let proc = self.zircon_process();
        proc.exit(-1);
        self.exit = true;
        Err(LxError::ENOSYS)
    }

    fn unimplemented(&self, name: &str, ret: SysResult) -> SysResult {
        warn!("{}: unimplemented", name);
        ret
    }

    fn zircon_process(&self) -> &Arc<Process> {
        self.thread.proc()
    }

    fn linux_process(&self) -> &LinuxProcess {
        self.zircon_process().linux()
    }
}
