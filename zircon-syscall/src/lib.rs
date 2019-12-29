#![no_std]
#![deny(unsafe_code, unused_must_use, unreachable_patterns)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use self::consts::*;
use crate::util::*;
use alloc::sync::Arc;
use alloc::vec::Vec;
use zircon_object::object::*;
use zircon_object::task::Thread;
use zircon_object::*;

mod channel;
mod consts;
mod debug;
mod task;
mod util;

pub struct Syscall {
    pub thread: Arc<Thread>,
}

impl Syscall {
    pub fn syscall(&self, num: u32, args: [usize; 8]) -> isize {
        info!("syscall => num={}, args={:x?}", num, args);
        let [a0, a1, a2, a3, a4, a5, a6, a7] = args;
        let ret = match num {
            SYS_CHANNEL_READ => self.sys_channel_read(
                a0 as _,
                a1 as _,
                a2.into(),
                a3.into(),
                a4 as _,
                a5 as _,
                a6.into(),
                a7.into(),
            ),
            SYS_DEBUG_WRITE => self.sys_debug_write(a0.into(), a1 as _),
            SYS_PROCESS_EXIT => self.sys_process_exit(a0 as _),
            _ => {
                warn!("syscall unimplemented");
                Err(ZxError::NOT_SUPPORTED)
            }
        };
        info!("syscall <= {:?}", ret);
        match ret {
            Ok(_) => 0,
            Err(err) => err as isize,
        }
    }
}
