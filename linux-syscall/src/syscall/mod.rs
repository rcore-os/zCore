use crate::fs::FileDesc;
use crate::process::{LinuxProcess, ProcessExt};
use spin::MutexGuard;
use zircon_object::task::Process;
use {
    self::consts::*, crate::error::*, crate::util::*, alloc::sync::Arc, zircon_object::hal,
    zircon_object::object::*, zircon_object::task::Thread,
};

mod consts;
mod file;
mod misc;
mod task;
mod vm;

pub struct Syscall {
    pub thread: Arc<Thread>,
}

impl Syscall {
    pub fn syscall(&self, num: u32, args: [usize; 6]) -> isize {
        info!("syscall => num={}, args={:x?}", num, args);
        let [a0, a1, a2, a3, a4, a5] = args;
        let ret = match num {
            SYS_READ => self.sys_read(a0.into(), a1.into(), a2),
            SYS_WRITE => self.sys_write(a0.into(), a1.into(), a2),
            SYS_PREAD64 => self.sys_pread(a0.into(), a1.into(), a2, a3),
            SYS_PWRITE64 => self.sys_pwrite(a0.into(), a1.into(), a2, a3),
            SYS_READV => self.sys_readv(a0.into(), a1.into(), a2),
            SYS_WRITEV => self.sys_writev(a0.into(), a1.into(), a2),
            SYS_OPENAT => self.sys_openat(a0.into(), a1.into(), a2, a3),
            SYS_CLOSE => self.sys_close(a0.into()),

            SYS_EXIT_GROUP => self.sys_exit_group(a0),

            SYS_MMAP => self.sys_mmap(a0, a1, a2, a3, a4.into(), a5),
            SYS_MPROTECT => self.sys_mprotect(a0, a1, a2),
            SYS_MUNMAP => self.sys_munmap(a0, a1),

            SYS_ARCH_PRCTL => self.sys_arch_prctl(a0 as _, a1 as _),
            SYS_SET_TID_ADDRESS => self.sys_set_tid_address(a0.into()),

            #[cfg(target_arch = "x86_64")]
            _ => self.x86_64_syscall(num, args),
        };
        info!("syscall <= {:?}", ret);
        match ret {
            Ok(value) => value as isize,
            Err(err) => -(err as isize),
        }
    }

    #[cfg(target_arch = "x86_64")]
    fn x86_64_syscall(&self, num: u32, args: [usize; 6]) -> SysResult {
        let [a0, a1, a2, a3, a4, a5] = args;
        match num {
            SYS_OPEN => self.sys_open(a0.into(), a1, a2),
            //            SYS_STAT => self.sys_stat(a0 as *const u8, a1 as *mut Stat),
            //            SYS_LSTAT => self.sys_lstat(a0 as *const u8, a1 as *mut Stat),
            //            SYS_POLL => self.sys_poll(a0 as *mut PollFd, a1, a2),
            //            SYS_ACCESS => self.sys_access(a0 as *const u8, a1),
            //            SYS_PIPE => self.sys_pipe(a0 as *mut u32),
            //            SYS_SELECT => self.sys_select(a0, a1 as *mut u32, a2 as *mut u32, a3 as *mut u32, a4 as *const TimeVal),
            SYS_DUP2 => self.sys_dup2(a0.into(), a1.into()),
            //            SYS_ALARM => self.unimplemented("alarm", Ok(0)),
            //            SYS_FORK => self.sys_fork(),
            //            SYS_VFORK => self.sys_vfork(),
            //            SYS_RENAME => self.sys_rename(a0 as *const u8, a1 as *const u8),
            //            SYS_MKDIR => self.sys_mkdir(a0 as *const u8, a1),
            //            SYS_RMDIR => self.sys_rmdir(a0 as *const u8),
            //            SYS_LINK => self.sys_link(a0 as *const u8, a1 as *const u8),
            //            SYS_UNLINK => self.sys_unlink(a0 as *const u8),
            //            SYS_READLINK => self.sys_readlink(a0 as *const u8, a1 as *mut u8, a2),
            //            SYS_CHMOD => self.unimplemented("chmod", Ok(0)),
            //            SYS_CHOWN => self.unimplemented("chown", Ok(0)),
            //            SYS_ARCH_PRCTL => self.sys_arch_prctl(a0 as i32, a1),
            //            SYS_TIME => self.sys_time(a0 as *mut u64),
            //            SYS_EPOLL_CREATE => self.sys_epoll_create(a0),
            //            SYS_EPOLL_WAIT => self.sys_epoll_wait(a0, a1 as *mut EpollEvent, a2, a3),
            _ => self.unknown_syscall(),
        }
    }

    fn unknown_syscall(&self) -> ! {
        warn!("syscall unimplemented! exit...");
        let proc = self.zircon_process();
        proc.exit(-1);
        Thread::exit();
    }

    fn zircon_process(&self) -> &Arc<Process> {
        &self.thread.proc
    }

    fn lock_linux_process(&self) -> MutexGuard<'_, LinuxProcess> {
        self.zircon_process().lock_linux()
    }
}
