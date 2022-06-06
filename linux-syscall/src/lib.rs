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
#[cfg(target_os = "none")]
use kernel_hal::vm::PagingError;
use kernel_hal::vm::PagingResult;
use kernel_hal::MMUFlags;
use linux_object::error::{LxError, SysResult};
use linux_object::fs::FileDesc;
use linux_object::process::{wait_child, wait_child_any, LinuxProcess, ProcessExt, RLimit};
use zircon_object::object::{KernelObject, KoID, Signal};
use zircon_object::task::{CurrentThread, Process, Thread, ThreadFn};
use zircon_object::{vm::VirtAddr, ZxError};

use self::consts::SyscallType as Sys;

mod consts {
    // generated from syscall.h.in
    include!(concat!(env!("OUT_DIR"), "/consts.rs"));
}
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
    #[cfg(not(target_os = "none"))]
    fn check_pagefault(&self, _vaddr: usize, _flags: MMUFlags) -> PagingResult<()> {
        Ok(())
    }

    #[cfg(target_os = "none")]
    fn check_pagefault(&self, vaddr: usize, flags: MMUFlags) -> PagingResult<()> {
        let vmar = self.thread.proc().vmar();
        if !vmar.contains(vaddr) {
            return Err(PagingError::NoMemory);
        }

        let mut is_handle_read_pagefault = flags.contains(MMUFlags::READ);
        let mut is_handle_write_pagefault = flags.contains(MMUFlags::WRITE);

        match vmar.get_vaddr_flags(vaddr) {
            Ok(vaddr_flags) => {
                is_handle_read_pagefault &= !vaddr_flags.contains(MMUFlags::READ);
                is_handle_write_pagefault &= !vaddr_flags.contains(MMUFlags::WRITE);
            }
            Err(PagingError::NotMapped) => {
                is_handle_read_pagefault &= true;
                is_handle_write_pagefault &= true;
            }
            Err(PagingError::NoMemory) => {
                error!("check_pagefault: vaddr(0x{:x}) NoMemory", vaddr);
                return Err(PagingError::NoMemory);
            }
            Err(PagingError::AlreadyMapped) => {
                panic!("get_vaddr_flags error!!!");
            }
        }

        if is_handle_read_pagefault {
            if let Err(err) = vmar.handle_page_fault(vaddr, MMUFlags::READ) {
                panic!("into_out_userptr handle_page_fault:  {:?}", err);
            }
        }

        if is_handle_write_pagefault {
            if let Err(err) = vmar.handle_page_fault(vaddr, MMUFlags::WRITE) {
                panic!("into_out_userptr handle_page_fault:  {:?}", err);
            }
        }
        Ok(())
    }

    /// convert a usize num to in and out userptr
    pub fn into_inout_userptr<T>(&self, vaddr: usize) -> PagingResult<UserInOutPtr<T>> {
        if 0 == vaddr {
            return Ok(vaddr.into());
        }

        let access_flags = MMUFlags::READ | MMUFlags::WRITE;
        self.check_pagefault(vaddr, access_flags)?;
        Ok(vaddr.into())
    }

    /// convert a usize num to in userptr
    pub fn into_in_userptr<T>(&self, vaddr: usize) -> PagingResult<UserInPtr<T>> {
        if 0 == vaddr {
            return Ok(vaddr.into());
        }

        self.check_pagefault(vaddr, MMUFlags::READ)?;
        Ok(vaddr.into())
    }

    /// convert a usize num to out userptr
    pub fn into_out_userptr<T>(&self, vaddr: usize) -> PagingResult<UserOutPtr<T>> {
        if 0 == vaddr {
            return Ok(vaddr.into());
        }

        self.check_pagefault(vaddr, MMUFlags::WRITE)?;
        Ok(vaddr.into())
    }

    /// syscall entry function
    pub async fn syscall(&mut self, num: u32, args: [usize; 6]) -> isize {
        trace!(
            "pid: {} syscall: num={}, args={:x?}",
            self.zircon_process().id(),
            num,
            args
        );
        let sys_type = match Sys::try_from(num) {
            Ok(t) => t,
            Err(_) => {
                error!("invalid syscall number: {}", num);
                return LxError::EINVAL as _;
            }
        };
        let [a0, a1, a2, a3, a4, a5] = args;
        let ret = match sys_type {
            Sys::READ => {
                self.sys_read(a0.into(), self.into_out_userptr(a1).unwrap(), a2)
                    .await
            }
            Sys::WRITE => self.sys_write(a0.into(), self.into_in_userptr(a1).unwrap(), a2),
            Sys::OPENAT => self.sys_openat(a0.into(), self.into_in_userptr(a1).unwrap(), a2, a3),
            Sys::CLOSE => self.sys_close(a0.into()),
            Sys::FSTAT => self.sys_fstat(a0.into(), self.into_out_userptr(a1).unwrap()),
            Sys::NEWFSTATAT => self.sys_fstatat(
                a0.into(),
                self.into_in_userptr(a1).unwrap(),
                self.into_out_userptr(a2).unwrap(),
                a3,
            ),
            Sys::LSEEK => self.sys_lseek(a0.into(), a1 as i64, a2 as u8),
            Sys::IOCTL => self.sys_ioctl(a0.into(), a1, a2, a3, a4),
            Sys::PREAD64 => {
                self.sys_pread(a0.into(), self.into_out_userptr(a1).unwrap(), a2, a3 as _)
                    .await
            }
            Sys::PWRITE64 => {
                self.sys_pwrite(a0.into(), self.into_in_userptr(a1).unwrap(), a2, a3 as _)
            }
            Sys::READV => {
                self.sys_readv(a0.into(), self.into_in_userptr(a1).unwrap(), a2)
                    .await
            }
            Sys::WRITEV => self.sys_writev(a0.into(), self.into_in_userptr(a1).unwrap(), a2),
            Sys::SENDFILE => {
                self.sys_sendfile(
                    a0.into(),
                    a1.into(),
                    self.into_inout_userptr(a2).unwrap(),
                    a3,
                )
                .await
            }
            Sys::FCNTL => self.sys_fcntl(a0.into(), a1, a2),
            Sys::FLOCK => self.sys_flock(a0.into(), a1),
            Sys::FSYNC => self.sys_fsync(a0.into()),
            Sys::FDATASYNC => self.sys_fdatasync(a0.into()),
            Sys::TRUNCATE => self.sys_truncate(self.into_in_userptr(a0).unwrap(), a1),
            Sys::FTRUNCATE => self.sys_ftruncate(a0.into(), a1),
            Sys::GETDENTS64 => {
                self.sys_getdents64(a0.into(), self.into_out_userptr(a1).unwrap(), a2)
            }
            Sys::GETCWD => self.sys_getcwd(self.into_out_userptr(a0).unwrap(), a1),
            Sys::CHDIR => self.sys_chdir(self.into_in_userptr(a0).unwrap()),
            Sys::RENAMEAT => self.sys_renameat(
                a0.into(),
                self.into_in_userptr(a1).unwrap(),
                a2.into(),
                self.into_in_userptr(a3).unwrap(),
            ),
            Sys::MKDIRAT => self.sys_mkdirat(a0.into(), self.into_in_userptr(a1).unwrap(), a2),
            Sys::LINKAT => self.sys_linkat(
                a0.into(),
                self.into_in_userptr(a1).unwrap(),
                a2.into(),
                self.into_in_userptr(a3).unwrap(),
                a4,
            ),
            Sys::UNLINKAT => self.sys_unlinkat(a0.into(), self.into_in_userptr(a1).unwrap(), a2),
            Sys::SYMLINKAT => self.unimplemented("symlinkat", Err(LxError::EACCES)),
            Sys::READLINKAT => self.sys_readlinkat(
                a0.into(),
                self.into_in_userptr(a1).unwrap(),
                self.into_out_userptr(a2).unwrap(),
                a3,
            ),
            Sys::FCHMOD => self.unimplemented("fchmod", Ok(0)),
            Sys::FCHMODAT => self.unimplemented("fchmodat", Ok(0)),
            Sys::FCHOWN => self.unimplemented("fchown", Ok(0)),
            Sys::FCHOWNAT => self.unimplemented("fchownat", Ok(0)),
            Sys::FACCESSAT => {
                self.sys_faccessat(a0.into(), self.into_in_userptr(a1).unwrap(), a2, a3)
            }
            Sys::DUP => self.sys_dup(a0.into()),
            Sys::DUP3 => self.sys_dup2(a0.into(), a1.into()), // TODO: handle `flags`
            Sys::PIPE2 => self.sys_pipe2(a0.into(), a1),      // TODO: handle `flags`
            Sys::UTIMENSAT => {
                self.sys_utimensat(a0.into(), self.into_in_userptr(a1).unwrap(), a2.into(), a3)
            }
            Sys::COPY_FILE_RANGE => {
                self.sys_copy_file_range(
                    a0.into(),
                    self.into_inout_userptr(a1).unwrap(),
                    a2.into(),
                    self.into_inout_userptr(a3).unwrap(),
                    a4,
                    a5,
                )
                .await
            }

            // io multiplexing
            Sys::PSELECT6 => {
                self.sys_pselect6(
                    a0,
                    self.into_inout_userptr(a1).unwrap(),
                    self.into_inout_userptr(a2).unwrap(),
                    self.into_inout_userptr(a3).unwrap(),
                    self.into_in_userptr(a4).unwrap(),
                    a5,
                )
                .await
            }
            Sys::PPOLL => {
                self.sys_ppoll(
                    self.into_inout_userptr(a0).unwrap(),
                    a1,
                    self.into_in_userptr(a2).unwrap(),
                )
                .await
            } // ignore sigmask
            //            Sys::EPOLL_CREATE1 => self.sys_epoll_create1(a0),
            //            Sys::EPOLL_CTL => self.sys_epoll_ctl(a0, a1, a2, a3.into()),
            //            Sys::EPOLL_PWAIT => self.sys_epoll_pwait(a0, a1.into(), a2, a3, a4),
            //            Sys::EVENTFD2 => self.unimplemented("eventfd2", Err(LxError::EACCES)),

            //            Sys::SOCKETPAIR => self.unimplemented("socketpair", Err(LxError::EACCES)),
            // file system
            Sys::STATFS => self.sys_statfs(
                self.into_in_userptr(a0).unwrap(),
                self.into_out_userptr(a1).unwrap(),
            ),
            Sys::FSTATFS => self.sys_fstatfs(a0.into(), self.into_out_userptr(a1).unwrap()),
            Sys::SYNC => self.sys_sync(),
            Sys::MOUNT => self.unimplemented("mount", Err(LxError::EACCES)),
            Sys::UMOUNT2 => self.unimplemented("umount2", Err(LxError::EACCES)),

            // memory
            Sys::BRK => self.unimplemented("brk", Err(LxError::ENOMEM)),
            Sys::MMAP => self.sys_mmap(a0, a1, a2, a3, a4.into(), a5 as _).await,
            Sys::MPROTECT => self.sys_mprotect(a0, a1, a2),
            Sys::MUNMAP => self.sys_munmap(a0, a1),
            Sys::MADVISE => self.unimplemented("madvise", Ok(0)),

            // signal
            Sys::RT_SIGACTION => self.sys_rt_sigaction(
                a0,
                self.into_in_userptr(a1).unwrap(),
                self.into_out_userptr(a2).unwrap(),
                a3,
            ),
            Sys::RT_SIGPROCMASK => self.sys_rt_sigprocmask(
                a0 as _,
                self.into_in_userptr(a1).unwrap(),
                self.into_out_userptr(a2).unwrap(),
                a3,
            ),
            Sys::RT_SIGRETURN => self.sys_rt_sigreturn(),
            Sys::SIGALTSTACK => self.sys_sigaltstack(
                self.into_in_userptr(a0).unwrap(),
                self.into_out_userptr(a1).unwrap(),
            ),
            Sys::KILL => self.sys_kill(a0 as isize, a1),

            // schedule
            Sys::SCHED_YIELD => self.unimplemented("yield", Ok(0)),
            Sys::SCHED_GETAFFINITY => self.unimplemented("sched_getaffinity", Ok(0)),
            Sys::SCHED_SETAFFINITY => self.unimplemented("sched_setaffinity", Ok(0)),

            // socket
            Sys::SOCKET => self.sys_socket(a0, a1, a2),
            Sys::CONNECT => self.sys_connect(a0, a1.into(), a2).await,
            Sys::ACCEPT => {
                self.sys_accept(
                    a0,
                    self.into_out_userptr(a1).unwrap(),
                    self.into_inout_userptr(a2).unwrap(),
                )
                .await
            }
            //            Sys::ACCEPT4 => self.sys_accept(a0, a1.into(), a2.into()), // use accept for accept4
            Sys::SENDTO => self.sys_sendto(
                a0,
                self.into_in_userptr(a1).unwrap(),
                a2,
                a3,
                self.into_in_userptr(a4).unwrap(),
                a5,
            ),
            Sys::RECVFROM => {
                self.sys_recvfrom(a0, a1.into(), a2, a3, a4.into(), a5.into())
                    .await
            }
            Sys::SENDMSG => self.unimplemented("sys_sendmsg(),", Ok(0)),
            Sys::RECVMSG => self.sys_recvmsg(a0, a1.into(), a2).await,
            Sys::SHUTDOWN => self.sys_shutdown(a0, a1),
            Sys::BIND => self.sys_bind(a0, self.into_in_userptr(a1).unwrap(), a2),
            Sys::LISTEN => self.sys_listen(a0, a1),

            Sys::GETSOCKNAME => self.sys_getsockname(
                a0,
                self.into_out_userptr(a1).unwrap(),
                self.into_inout_userptr(a2).unwrap(),
            ),
            Sys::GETPEERNAME => self.sys_getpeername(
                a0,
                self.into_out_userptr(a1).unwrap(),
                self.into_inout_userptr(a2).unwrap(),
            ),
            Sys::SETSOCKOPT => {
                self.sys_setsockopt(a0, a1, a2, self.into_in_userptr(a3).unwrap(), a4)
            }
            Sys::GETSOCKOPT => {
                self.sys_getsockopt(a0, a1, a2, self.into_out_userptr(a3).unwrap(), a4)
            }

            // process
            Sys::EXECVE => self.sys_execve(
                self.into_in_userptr(a0).unwrap(),
                self.into_in_userptr(a1).unwrap(),
                self.into_in_userptr(a2).unwrap(),
            ),
            Sys::EXIT => self.sys_exit(a0 as _),
            Sys::EXIT_GROUP => self.sys_exit_group(a0 as _),
            Sys::WAIT4 => {
                self.sys_wait4(a0 as _, self.into_out_userptr(a1).unwrap(), a2 as _)
                    .await
            }
            Sys::SET_TID_ADDRESS => self.sys_set_tid_address(self.into_out_userptr(a0).unwrap()),
            Sys::FUTEX => self.sys_futex(a0, a1 as _, a2 as _, a3, a4, a5 as _).await,
            Sys::GET_ROBUST_LIST => self.sys_get_robust_list(
                a0 as _,
                self.into_out_userptr(a1).unwrap(),
                self.into_out_userptr(a2).unwrap(),
            ),
            Sys::SET_ROBUST_LIST => {
                self.sys_set_robust_list(self.into_in_userptr(a0).unwrap(), a1 as _)
            }
            Sys::TKILL => self.sys_tkill(a0, a1),
            Sys::TGKILL => self.sys_tgkill(a0, a1, a2),

            // time
            Sys::NANOSLEEP => self.sys_nanosleep(self.into_in_userptr(a0).unwrap()).await,
            Sys::CLOCK_NANOSLEEP => {
                self.sys_clock_nanosleep(
                    a0,
                    a1,
                    self.into_in_userptr(a2).unwrap(),
                    self.into_out_userptr(a3).unwrap(),
                )
                .await
            }
            Sys::SETITIMER => self.unimplemented("setitimer", Ok(0)),
            Sys::GETTIMEOFDAY => self.sys_gettimeofday(
                self.into_out_userptr(a0).unwrap(),
                self.into_in_userptr(a1).unwrap(),
            ),
            Sys::CLOCK_GETTIME => self.sys_clock_gettime(a0, self.into_out_userptr(a1).unwrap()),
            Sys::CLOCK_GETRES => self.unimplemented("clock_getres", Ok(0)),

            // sem
            #[cfg(not(target_arch = "mips"))]
            Sys::SEMGET => self.sys_semget(a0, a1, a2),
            #[cfg(not(target_arch = "mips"))]
            Sys::SEMOP => {
                self.sys_semop(a0, self.into_in_userptr(a1).unwrap(), a2)
                    .await
            }
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
            Sys::UNAME => self.sys_uname(self.into_out_userptr(a0).unwrap()),
            Sys::UMASK => self.unimplemented("umask", Ok(0o777)),
            //            Sys::GETRLIMIT => self.sys_getrlimit(),
            //            Sys::SETRLIMIT => self.sys_setrlimit(),
            Sys::GETRUSAGE => self.sys_getrusage(a0, self.into_out_userptr(a1).unwrap()),
            Sys::SYSINFO => self.sys_sysinfo(self.into_out_userptr(a0).unwrap()),
            Sys::TIMES => self.sys_times(self.into_out_userptr(a0).unwrap()),
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
            Sys::PRLIMIT64 => self.sys_prlimit64(
                a0,
                a1,
                self.into_in_userptr(a2).unwrap(),
                self.into_out_userptr(a3).unwrap(),
            ),
            //            Sys::REBOOT => self.sys_reboot(a0 as u32, a1 as u32, a2 as u32, a3.into()),
            Sys::GETRANDOM => {
                self.sys_getrandom(self.into_out_userptr(a0).unwrap(), a1 as usize, a2 as u32)
            }
            Sys::RT_SIGQUEUEINFO => self.unimplemented("rt_sigqueueinfo", Ok(0)),

            // kernel module
            //            Sys::INIT_MODULE => self.sys_init_module(a0.into(), a1 as usize, a2.into()),
            Sys::FINIT_MODULE => self.unimplemented("finit_module", Err(LxError::ENOSYS)),
            //            Sys::DELETE_MODULE => self.sys_delete_module(a0.into(), a1 as u32),
            #[cfg(not(target_arch = "aarch64"))]
            Sys::BLOCK_IN_KERNEL => self.sys_block_in_kernel(),

            #[cfg(target_arch = "x86_64")]
            _ => self.x86_64_syscall(sys_type, args).await,
            #[cfg(target_arch = "riscv64")]
            _ => self.riscv64_syscall(sys_type, args).await,
            #[cfg(target_arch = "aarch64")]
            _ => self.aarch64_syscall(sys_type, args).await,
        };
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
            Sys::CLONE => self.sys_clone(a0, a1, a2.into(), a3, a4.into()),
            _ => self.unknown_syscall(sys_type),
        }
    }

    #[cfg(target_arch = "x86_64")]
    /// syscall specified for x86_64
    async fn x86_64_syscall(&mut self, sys_type: Sys, args: [usize; 6]) -> SysResult {
        let [a0, a1, a2, a3, a4, _a5] = args;
        match sys_type {
            Sys::OPEN => self.sys_open(self.into_in_userptr(a0).unwrap(), a1, a2),
            Sys::STAT => self.sys_stat(
                self.into_in_userptr(a0).unwrap(),
                self.into_out_userptr(a1).unwrap(),
            ),
            Sys::LSTAT => self.sys_lstat(
                self.into_in_userptr(a0).unwrap(),
                self.into_out_userptr(a1).unwrap(),
            ),
            Sys::POLL => {
                self.sys_poll(self.into_inout_userptr(a0).unwrap(), a1, a2 as _)
                    .await
            }
            Sys::ACCESS => self.sys_access(self.into_in_userptr(a0).unwrap(), a1),
            Sys::PIPE => self.sys_pipe(self.into_out_userptr(a0).unwrap()),
            Sys::SELECT => {
                self.sys_select(
                    a0,
                    self.into_inout_userptr(a1).unwrap(),
                    self.into_inout_userptr(a2).unwrap(),
                    self.into_inout_userptr(a3).unwrap(),
                    self.into_in_userptr(a4).unwrap(),
                )
                .await
            }
            Sys::DUP2 => self.sys_dup2(a0.into(), a1.into()),
            //            Sys::ALARM => self.unimplemented("alarm", Ok(0)),
            Sys::FORK => self.sys_fork(),
            Sys::VFORK => self.sys_vfork().await,
            Sys::RENAME => self.sys_rename(
                self.into_in_userptr(a0).unwrap(),
                self.into_in_userptr(a1).unwrap(),
            ),
            Sys::MKDIR => self.sys_mkdir(self.into_in_userptr(a0).unwrap(), a1),
            Sys::RMDIR => self.sys_rmdir(self.into_in_userptr(a0).unwrap()),
            Sys::LINK => self.sys_link(
                self.into_in_userptr(a0).unwrap(),
                self.into_in_userptr(a1).unwrap(),
            ),
            Sys::UNLINK => self.sys_unlink(self.into_in_userptr(a0).unwrap()),
            Sys::READLINK => self.sys_readlink(
                self.into_in_userptr(a0).unwrap(),
                self.into_out_userptr(a1).unwrap(),
                a2,
            ),
            Sys::CHMOD => self.unimplemented("chmod", Ok(0)),
            Sys::CHOWN => self.unimplemented("chown", Ok(0)),
            Sys::ARCH_PRCTL => self.sys_arch_prctl(a0 as _, a1),
            Sys::TIME => self.sys_time(self.into_out_userptr(a0).unwrap()),
            Sys::CLONE => self.sys_clone(a0, a1, a2.into(), a4, a3.into()),
            //            Sys::EPOLL_CREATE => self.sys_epoll_create(a0),
            //            Sys::EPOLL_WAIT => self.sys_epoll_wait(a0, a1.into(), a2, a3),
            _ => self.unknown_syscall(sys_type),
        }
    }

    #[cfg(target_arch = "riscv64")]
    async fn riscv64_syscall(&mut self, sys_type: Sys, args: [usize; 6]) -> SysResult {
        let [a0, a1, a2, a3, a4, _a5] = args;
        match sys_type {
            //Sys::OPEN => self.sys_open(a0.into(), a1, a2),
            Sys::CLONE => self.sys_clone(a0, a1, a2.into(), a3, a4.into()),
            _ => self.unknown_syscall(sys_type),
        }
    }

    /// unkown syscalls, currently is similar to unimplemented syscalls but emit an error
    fn unknown_syscall(&mut self, sys_type: Sys) -> SysResult {
        error!("unknown syscall: {:?}. exit...", sys_type);
        let proc = self.zircon_process();
        proc.exit(-1);
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
