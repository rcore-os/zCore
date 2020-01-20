#![no_std]
#![deny(warnings, unsafe_code, unused_must_use, unreachable_patterns)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use {
    self::consts::*, crate::util::*, alloc::sync::Arc, alloc::vec::Vec, zircon_object::object::*,
    zircon_object::task::Thread,
};

mod channel;
mod consts;
mod debug;
mod debuglog;
mod handle;
mod task;
mod util;
mod vmar;
mod vmo;

pub struct Syscall {
    pub thread: Arc<Thread>,
}

impl Syscall {
    pub fn syscall(&self, num: u32, args: [usize; 8]) -> isize {
        info!("syscall => num={}, args={:x?}", num, args);
        let [a0, a1, a2, a3, a4, a5, a6, a7] = args;
        let ret = match num {
            SYS_HANDLE_DUPLICATE => self.sys_handle_duplicate(a0 as _, a1 as _, a2.into()),
            SYS_HANDLE_CLOSE => self.sys_handle_close(a0 as _),
            SYS_HANDLE_CLOSE_MANY => self.sys_handle_close_many(a0.into(), a1 as _),
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
            SYS_PROCESS_CREATE => {
                self.sys_process_create(a0 as _, a1.into(), a2 as _, a3 as _, a4.into(), a5.into())
            }
            SYS_PROCESS_EXIT => self.sys_process_exit(a0 as _),
            SYS_DEBUGLOG_CREATE => self.sys_debuglog_create(a0 as _, a1 as _, a2.into()),
            SYS_DEBUGLOG_WRITE => self.sys_debuglog_write(a0 as _, a1 as _, a2.into(), a3 as _),
            SYS_VMO_CREATE => self.sys_vmo_create(a0 as _, a1 as _, a2.into()),
            SYS_VMO_READ => self.sys_vmo_read(a0 as _, a1.into(), a2 as _, a3 as _),
            SYS_VMAR_ALLOCATE => {
                self.sys_vmar_allocate(a0 as _, a1 as _, a2 as _, a3 as _, a4.into(), a5.into())
            }
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
