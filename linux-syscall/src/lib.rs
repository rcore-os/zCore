//! Linux syscall implementations

#![no_std]
#![deny(warnings, unsafe_code, unused_must_use, unreachable_patterns)]

//#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use zircon_object::task::Process;
use {
    crate::consts::*, crate::error::*, crate::util::*, alloc::sync::Arc, zircon_object::hal,
    zircon_object::object::*, zircon_object::task::Thread,
};

mod consts;
mod error;
mod file;
mod fs;
mod mem;
mod proc;
mod util;

pub struct Syscall {
    pub thread: Arc<Thread>,
}

impl Syscall {
    pub fn syscall(&self, num: u32, args: [usize; 6]) -> isize {
        info!("syscall => num={}, args={:x?}", num, args);
        let [a0, a1, a2, a3, a4, a5] = args;
        let ret = match num {
            SYS_EXIT_GROUP => self.sys_exit_group(a0),

            SYS_WRITEV => self.sys_writev(a0 as _, a1.into(), a2),

            SYS_MMAP => self.sys_mmap(a0, a1, a2, a3, a4 as _, a5),
            SYS_MPROTECT => self.sys_mprotect(a0, a1, a2),
            SYS_MUNMAP => self.sys_munmap(a0, a1),

            SYS_ARCH_PRCTL => self.sys_arch_prctl(a0 as _, a1 as _),
            SYS_SET_TID_ADDRESS => self.sys_set_tid_address(a0.into()),
            _ => {
                warn!("syscall unimplemented! exit...");
                let proc = self.process();
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

    fn process(&self) -> &Arc<Process> {
        &self.thread.proc
    }

    fn sys_arch_prctl(&self, code: i32, addr: usize) -> SysResult<usize> {
        const ARCH_SET_FS: i32 = 0x1002;
        match code {
            ARCH_SET_FS => {
                info!("sys_arch_prctl: set FSBASE to {:#x}", addr);
                hal::set_user_fsbase(addr);
                Ok(0)
            }
            _ => Err(SysError::EINVAL),
        }
    }

    fn sys_set_tid_address(&self, tidptr: UserOutPtr<u32>) -> SysResult<usize> {
        warn!("set_tid_address: {:?}. unimplemented!", tidptr);
        //        self.thread.clear_child_tid = tidptr as usize;
        let tid = self.thread.id();
        Ok(tid as usize)
    }
}

type FileDesc = isize;
