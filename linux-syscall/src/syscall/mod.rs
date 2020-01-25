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

            SYS_EXIT_GROUP => self.sys_exit_group(a0),

            SYS_MMAP => self.sys_mmap(a0, a1, a2, a3, a4.into(), a5),
            SYS_MPROTECT => self.sys_mprotect(a0, a1, a2),
            SYS_MUNMAP => self.sys_munmap(a0, a1),

            SYS_ARCH_PRCTL => self.sys_arch_prctl(a0 as _, a1 as _),
            SYS_SET_TID_ADDRESS => self.sys_set_tid_address(a0.into()),
            _ => {
                warn!("syscall unimplemented! exit...");
                let proc = self.zircon_process();
                proc.exit(-1);
                Thread::exit();
            }
        };
        info!("syscall <= {:?}", ret);
        match ret {
            Ok(value) => value as isize,
            Err(err) => -(err as isize),
        }
    }

    fn zircon_process(&self) -> &Arc<Process> {
        &self.thread.proc
    }

    fn process(&self) -> MutexGuard<'_, LinuxProcess> {
        self.zircon_process().lock_linux()
    }
}
