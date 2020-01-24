//! Linux syscall implementations

#![no_std]
#![deny(warnings, unsafe_code, unused_must_use, unreachable_patterns)]

//#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use {
    crate::consts::*, crate::error::*, crate::util::*, alloc::sync::Arc, zircon_object::hal,
    zircon_object::object::*, zircon_object::task::Thread,
};

mod consts;
mod error;
mod mem;
mod util;

pub struct Syscall {
    pub thread: Arc<Thread>,
}

impl Syscall {
    pub fn syscall(&self, num: u32, args: [usize; 6]) -> isize {
        info!("syscall => num={}, args={:x?}", num, args);
        let [a0, a1, a2, a3, a4, a5] = args;
        let ret = match num {
            SYS_MMAP => self.sys_mmap(a0, a1, a2, a3, a4 as _, a5),
            SYS_MPROTECT => self.sys_mprotect(a0, a1, a2),
            SYS_MUNMAP => self.sys_munmap(a0, a1),
            SYS_ARCH_PRCTL => self.sys_arch_prctl(a0 as _, a1 as _),
            SYS_SET_TID_ADDRESS => self.sys_set_tid_address(a0.into()),
            _ => {
                warn!("syscall unimplemented! exit...");
                let proc = &self.thread.proc;
                proc.exit(-1);
                Thread::exit();
            }
        };
        info!("syscall <= {:?}", ret);
        match ret {
            Ok(_) => 0,
            Err(err) => -(err as isize),
        }
    }

    fn sys_arch_prctl(&self, code: i32, addr: usize) -> SysResult {
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

    fn sys_set_tid_address(&self, tidptr: UserOutPtr<u32>) -> SysResult {
        warn!("set_tid_address: {:?}. unimplemented!", tidptr);
        //        self.thread.clear_child_tid = tidptr as usize;
        let tid = self.thread.id();
        Ok(tid as usize)
    }
}

type FileDesc = isize;
