//! Linux syscall implementations

#![no_std]
#![deny(warnings, unsafe_code, unused_must_use, unreachable_patterns)]
#![feature(bool_to_option)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use {
    self::{consts::*, util::*},
    alloc::sync::Arc,
    kernel_hal::{user::*, GeneralRegs},
    linux_object::{error::*, fs::FileDesc, process::*},
    spin::MutexGuard,
    zircon_object::{object::*, task::*, vm::VirtAddr},
};

mod consts;
mod file;
mod misc;
mod task;
mod time;
mod util;
mod vm;

pub struct Syscall<'a> {
    pub thread: &'a Arc<Thread>,
    pub syscall_entry: VirtAddr,
    pub regs: &'a mut GeneralRegs,
    pub spawn_fn: fn(thread: Arc<Thread>),
    /// Set `true` to exit current task.
    pub exit: bool,
}

impl Syscall<'_> {
    pub async fn syscall(&mut self, num: u32, args: [usize; 6]) -> isize {
        debug!("syscall => num={}, args={:x?}", num, args);
        let [a0, a1, a2, a3, a4, a5] = args;
        let ret = match num {
            SYS_READ => self.sys_read(a0.into(), a1.into(), a2),
            SYS_WRITE => self.sys_write(a0.into(), a1.into(), a2),
            SYS_OPENAT => self.sys_openat(a0.into(), a1.into(), a2, a3),
            SYS_CLOSE => self.sys_close(a0.into()),
            SYS_FSTAT => self.sys_fstat(a0.into(), a1.into()),
            SYS_NEWFSTATAT => self.sys_fstatat(a0.into(), a1.into(), a2.into(), a3),
            SYS_LSEEK => self.sys_lseek(a0.into(), a1 as i64, a2 as u8),
            SYS_IOCTL => self.sys_ioctl(a0.into(), a1, a2, a3, a4),
            SYS_PREAD64 => self.sys_pread(a0.into(), a1.into(), a2, a3 as _),
            SYS_PWRITE64 => self.sys_pwrite(a0.into(), a1.into(), a2, a3 as _),
            SYS_READV => self.sys_readv(a0.into(), a1.into(), a2),
            SYS_WRITEV => self.sys_writev(a0.into(), a1.into(), a2),
            SYS_SENDFILE => self.sys_sendfile(a0.into(), a1.into(), a2.into(), a3),
            SYS_FCNTL => self.sys_fcntl(a0.into(), a1, a2),
            SYS_FLOCK => self.unimplemented("flock", Ok(0)),
            SYS_FSYNC => self.sys_fsync(a0.into()),
            SYS_FDATASYNC => self.sys_fdatasync(a0.into()),
            SYS_TRUNCATE => self.sys_truncate(a0.into(), a1),
            SYS_FTRUNCATE => self.sys_ftruncate(a0.into(), a1),
            SYS_GETDENTS64 => self.sys_getdents64(a0.into(), a1.into(), a2),
            SYS_GETCWD => self.sys_getcwd(a0.into(), a1),
            SYS_CHDIR => self.sys_chdir(a0.into()),
            SYS_RENAMEAT => self.sys_renameat(a0.into(), a1.into(), a2.into(), a3.into()),
            SYS_MKDIRAT => self.sys_mkdirat(a0.into(), a1.into(), a2),
            SYS_LINKAT => self.sys_linkat(a0.into(), a1.into(), a2.into(), a3.into(), a4),
            SYS_UNLINKAT => self.sys_unlinkat(a0.into(), a1.into(), a2),
            SYS_SYMLINKAT => self.unimplemented("symlinkat", Err(LxError::EACCES)),
            SYS_READLINKAT => self.sys_readlinkat(a0.into(), a1.into(), a2.into(), a3),
            SYS_FCHMOD => self.unimplemented("fchmod", Ok(0)),
            SYS_FCHMODAT => self.unimplemented("fchmodat", Ok(0)),
            SYS_FCHOWN => self.unimplemented("fchown", Ok(0)),
            SYS_FCHOWNAT => self.unimplemented("fchownat", Ok(0)),
            SYS_FACCESSAT => self.sys_faccessat(a0.into(), a1.into(), a2, a3),
            SYS_DUP3 => self.sys_dup2(a0.into(), a1.into()), // TODO: handle `flags`
            //            SYS_PIPE2 => self.sys_pipe(a0.into()),           // TODO: handle `flags`
            SYS_UTIMENSAT => self.unimplemented("utimensat", Ok(0)),
            SYS_COPY_FILE_RANGE => {
                self.sys_copy_file_range(a0.into(), a1.into(), a2.into(), a3.into(), a4, a5)
            }

            // io multiplexing
            //            SYS_PSELECT6 => self.sys_pselect6(a0, a1.into(), a2.into(), a3.into(), a4.into(), a5.into()),
            //            SYS_PPOLL => self.sys_ppoll(a0.into(), a1, a2.into()), // ignore sigmask
            //            SYS_EPOLL_CREATE1 => self.sys_epoll_create1(a0),
            //            SYS_EPOLL_CTL => self.sys_epoll_ctl(a0, a1, a2, a3.into()),
            //            SYS_EPOLL_PWAIT => self.sys_epoll_pwait(a0, a1.into(), a2, a3, a4),
            //            SYS_EVENTFD2 => self.unimplemented("eventfd2", Err(LxError::EACCES)),

            //            SYS_SOCKETPAIR => self.unimplemented("socketpair", Err(LxError::EACCES)),
            // file system
            SYS_STATFS => self.unimplemented("statfs", Err(LxError::EACCES)),
            SYS_FSTATFS => self.unimplemented("fstatfs", Err(LxError::EACCES)),
            SYS_SYNC => self.sys_sync(),
            SYS_MOUNT => self.unimplemented("mount", Err(LxError::EACCES)),
            SYS_UMOUNT2 => self.unimplemented("umount2", Err(LxError::EACCES)),

            // memory
            SYS_BRK => self.unimplemented("brk", Err(LxError::ENOMEM)),
            SYS_MMAP => self.sys_mmap(a0, a1, a2, a3, a4.into(), a5 as _),
            SYS_MPROTECT => self.sys_mprotect(a0, a1, a2),
            SYS_MUNMAP => self.sys_munmap(a0, a1),
            SYS_MADVISE => self.unimplemented("madvise", Ok(0)),

            // signal
            SYS_RT_SIGACTION => self.unimplemented("sigaction", Ok(0)),
            SYS_RT_SIGPROCMASK => self.unimplemented("sigprocmask", Ok(0)),
            SYS_SIGALTSTACK => self.unimplemented("sigaltstack", Ok(0)),
            //            SYS_KILL => self.sys_kill(a0, a1),

            // schedule
            //            SYS_SCHED_YIELD => self.sys_yield(),
            //            SYS_SCHED_GETAFFINITY => self.sys_sched_getaffinity(a0, a1, a2.into()),

            // socket
            //            SYS_SOCKET => self.sys_socket(a0, a1, a2),
            //            SYS_CONNECT => self.sys_connect(a0, a1.into(), a2),
            //            SYS_ACCEPT => self.sys_accept(a0, a1.into(), a2.into()),
            //            SYS_ACCEPT4 => self.sys_accept(a0, a1.into(), a2.into()), // use accept for accept4
            //            SYS_SENDTO => self.sys_sendto(a0, a1.into(), a2, a3, a4.into(), a5),
            //            SYS_RECVFROM => self.sys_recvfrom(a0, a1.into(), a2, a3, a4.into(), a5.into()),
            //            SYS_SENDMSG => self.sys_sendmsg(),
            //            SYS_RECVMSG => self.sys_recvmsg(a0, a1.into(), a2),
            //            SYS_SHUTDOWN => self.sys_shutdown(a0, a1),
            //            SYS_BIND => self.sys_bind(a0, a1.into(), a2),
            //            SYS_LISTEN => self.sys_listen(a0, a1),
            //            SYS_GETSOCKNAME => self.sys_getsockname(a0, a1.into(), a2.into()),
            //            SYS_GETPEERNAME => self.sys_getpeername(a0, a1.into(), a2.into()),
            //            SYS_SETSOCKOPT => self.sys_setsockopt(a0, a1, a2, a3.into(), a4),
            //            SYS_GETSOCKOPT => self.sys_getsockopt(a0, a1, a2, a3.into(), a4.into()),

            // process
            SYS_CLONE => self.sys_clone(a0, a1, a2.into(), a3.into(), a4),
            SYS_EXECVE => self.sys_execve(a0.into(), a1.into(), a2.into()),
            SYS_EXIT => self.sys_exit(a0 as _),
            SYS_EXIT_GROUP => self.sys_exit_group(a0 as _),
            SYS_WAIT4 => self.sys_wait4(a0 as _, a1.into(), a2 as _).await,
            SYS_SET_TID_ADDRESS => self.sys_set_tid_address(a0.into()),
            SYS_FUTEX => self.sys_futex(a0, a1 as _, a2 as _, a3.into()).await,
            SYS_TKILL => self.unimplemented("tkill", Ok(0)),

            // time
            //            SYS_NANOSLEEP => self.sys_nanosleep(a0.into()),
            SYS_SETITIMER => self.unimplemented("setitimer", Ok(0)),
            //            SYS_GETTIMEOFDAY => self.sys_gettimeofday(a0.into(), a1.into()),
            //            SYS_CLOCK_GETTIME => self.sys_clock_gettime(a0, a1.into()),

            // sem
            //            #[cfg(not(target_arch = "mips"))]
            //            SYS_SEMGET => self.sys_semget(a0, a1 as isize, a2),
            //            #[cfg(not(target_arch = "mips"))]
            //            SYS_SEMOP => self.sys_semop(a0, a1.into(), a2),
            //            #[cfg(not(target_arch = "mips"))]
            //            SYS_SEMCTL => self.sys_semctl(a0, a1, a2, a3 as isize),

            // system
            SYS_GETPID => self.sys_getpid(),
            SYS_GETTID => self.sys_gettid(),
            SYS_UNAME => self.sys_uname(a0.into()),
            SYS_UMASK => self.unimplemented("umask", Ok(0o777)),
            //            SYS_GETRLIMIT => self.sys_getrlimit(),
            //            SYS_SETRLIMIT => self.sys_setrlimit(),
            //            SYS_GETRUSAGE => self.sys_getrusage(a0, a1.into()),
            //            SYS_SYSINFO => self.sys_sysinfo(a0.into()),
            //            SYS_TIMES => self.sys_times(a0.into()),
            SYS_GETUID => self.unimplemented("getuid", Ok(0)),
            SYS_GETGID => self.unimplemented("getgid", Ok(0)),
            SYS_SETUID => self.unimplemented("setuid", Ok(0)),
            SYS_GETEUID => self.unimplemented("geteuid", Ok(0)),
            SYS_GETEGID => self.unimplemented("getegid", Ok(0)),
            SYS_SETPGID => self.unimplemented("setpgid", Ok(0)),
            SYS_GETPPID => self.sys_getppid(),
            SYS_SETSID => self.unimplemented("setsid", Ok(0)),
            SYS_GETPGID => self.unimplemented("getpgid", Ok(0)),
            SYS_GETGROUPS => self.unimplemented("getgroups", Ok(0)),
            SYS_SETGROUPS => self.unimplemented("setgroups", Ok(0)),
            //            SYS_SETPRIORITY => self.sys_set_priority(a0),
            SYS_PRCTL => self.unimplemented("prctl", Ok(0)),
            SYS_MEMBARRIER => self.unimplemented("membarrier", Ok(0)),
            //            SYS_PRLIMIT64 => self.sys_prlimit64(a0, a1, a2.into(), a3.into()),
            //            SYS_REBOOT => self.sys_reboot(a0 as u32, a1 as u32, a2 as u32, a3.into()),
            //            SYS_GETRANDOM => self.sys_getrandom(a0.into(), a1 as usize, a2 as u32),
            SYS_RT_SIGQUEUEINFO => self.unimplemented("rt_sigqueueinfo", Ok(0)),

            // kernel module
            //            SYS_INIT_MODULE => self.sys_init_module(a0.into(), a1 as usize, a2.into()),
            SYS_FINIT_MODULE => self.unimplemented("finit_module", Err(LxError::ENOSYS)),
            //            SYS_DELETE_MODULE => self.sys_delete_module(a0.into(), a1 as u32),
            #[cfg(target_arch = "x86_64")]
            _ => self.x86_64_syscall(num, args).await,
        };
        info!("<= {:x?}", ret);
        match ret {
            Ok(value) => value as isize,
            Err(err) => -(err as isize),
        }
    }

    #[cfg(target_arch = "x86_64")]
    async fn x86_64_syscall(&mut self, num: u32, args: [usize; 6]) -> SysResult {
        let [a0, a1, a2, _a3, _a4, _a5] = args;
        match num {
            SYS_OPEN => self.sys_open(a0.into(), a1, a2),
            SYS_STAT => self.sys_stat(a0.into(), a1.into()),
            SYS_LSTAT => self.sys_lstat(a0.into(), a1.into()),
            //            SYS_POLL => self.sys_poll(a0.into(), a1, a2),
            SYS_ACCESS => self.sys_access(a0.into(), a1),
            //            SYS_PIPE => self.sys_pipe(a0.into()),
            //            SYS_SELECT => self.sys_select(a0, a1.into(), a2.into(), a3.into(), a4.into()),
            SYS_DUP2 => self.sys_dup2(a0.into(), a1.into()),
            //            SYS_ALARM => self.unimplemented("alarm", Ok(0)),
            SYS_FORK => self.sys_fork().await,
            SYS_VFORK => self.sys_vfork().await,
            SYS_RENAME => self.sys_rename(a0.into(), a1.into()),
            SYS_MKDIR => self.sys_mkdir(a0.into(), a1),
            SYS_RMDIR => self.sys_rmdir(a0.into()),
            SYS_LINK => self.sys_link(a0.into(), a1.into()),
            SYS_UNLINK => self.sys_unlink(a0.into()),
            SYS_READLINK => self.sys_readlink(a0.into(), a1.into(), a2),
            //            SYS_CHMOD => self.unimplemented("chmod", Ok(0)),
            //            SYS_CHOWN => self.unimplemented("chown", Ok(0)),
            SYS_ARCH_PRCTL => self.sys_arch_prctl(a0 as _, a1),
            //            SYS_TIME => self.sys_time(a0 as *mut u64),
            //            SYS_EPOLL_CREATE => self.sys_epoll_create(a0),
            //            SYS_EPOLL_WAIT => self.sys_epoll_wait(a0, a1.into(), a2, a3),
            _ => self.unknown_syscall(num),
        }
    }

    fn unknown_syscall(&mut self, num: u32) -> SysResult {
        error!("unknown syscall: {}. exit...", num);
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

    fn lock_linux_process(&self) -> MutexGuard<'_, LinuxProcess> {
        self.zircon_process().lock_linux()
    }
}
