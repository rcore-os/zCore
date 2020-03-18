//! Zircon syscall implementations

#![no_std]
#![deny(warnings, unsafe_code, unused_must_use, unreachable_patterns)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use {
    alloc::sync::Arc,
    alloc::vec::Vec,
    kernel_hal::{user::*, GeneralRegs},
    zircon_object::object::*,
    zircon_object::task::Thread,
};

mod channel;
mod clock;
mod consts;
mod cprng;
mod debug;
mod debuglog;
mod handle;
mod object;
mod resource;
mod signal;
mod suspend_task;
mod task;
mod vmar;
mod vmo;

pub use consts::SyscallType;

pub struct Syscall<'a> {
    pub regs: &'a mut GeneralRegs,
    pub thread: Arc<Thread>,
    pub exit: bool,
}

impl Syscall<'_> {
    pub async fn syscall(&mut self, sys_type: SyscallType, args: [usize; 8]) -> isize {
        let thread_name = self.thread.name();
        info!("{} {:?}=> args={:x?}", thread_name, sys_type, args);
        let [a0, a1, a2, a3, a4, a5, a6, a7] = args;
        let ret = match sys_type {
            SyscallType::HANDLE_DUPLICATE => self.sys_handle_duplicate(a0 as _, a1 as _, a2.into()),
            SyscallType::HANDLE_CLOSE => self.sys_handle_close(a0 as _),
            SyscallType::HANDLE_CLOSE_MANY => self.sys_handle_close_many(a0.into(), a1 as _),
            SyscallType::CHANNEL_READ => self.sys_channel_read(
                a0 as _,
                a1 as _,
                a2.into(),
                a3.into(),
                a4 as _,
                a5 as _,
                a6.into(),
                a7.into(),
            ),
            SyscallType::OBJECT_GET_PROPERTY => {
                self.sys_object_get_property(a0 as _, a1 as _, a2 as _, a3 as _)
            }
            SyscallType::OBJECT_SET_PROPERTY => {
                self.sys_object_set_property(a0 as _, a1 as _, a2 as _, a3 as _)
            }
            SyscallType::DEBUG_WRITE => self.sys_debug_write(a0.into(), a1 as _),
            SyscallType::PROCESS_CREATE => {
                self.sys_process_create(a0 as _, a1.into(), a2 as _, a3 as _, a4.into(), a5.into())
            }
            SyscallType::PROCESS_EXIT => self.sys_process_exit(a0 as _),
            SyscallType::DEBUGLOG_CREATE => self.sys_debuglog_create(a0 as _, a1 as _, a2.into()),
            SyscallType::DEBUGLOG_WRITE => {
                self.sys_debuglog_write(a0 as _, a1 as _, a2.into(), a3 as _)
            }
            SyscallType::VMO_CREATE => self.sys_vmo_create(a0 as _, a1 as _, a2.into()),
            SyscallType::VMO_READ => self.sys_vmo_read(a0 as _, a1.into(), a2 as _, a3 as _),
            SyscallType::VMO_WRITE => self.sys_vmo_write(a0 as _, a1.into(), a2 as _, a3 as _),
            SyscallType::VMAR_MAP => self.sys_vmar_map(
                a0 as _,
                a1 as _,
                a2 as _,
                a3 as _,
                a4 as _,
                a5 as _,
                a6.into(),
            ),
            SyscallType::VMAR_ALLOCATE => {
                self.sys_vmar_allocate(a0 as _, a1 as _, a2 as _, a3 as _, a4.into(), a5.into())
            }
            SyscallType::CPRNG_DRAW_ONCE => self.sys_cprng_draw_once(a0 as _, a1 as _),
            SyscallType::THREAD_CREATE => {
                self.sys_thread_create(a0 as _, a1.into(), a2 as _, a3 as _, a4.into())
            }
            SyscallType::TASK_SUSPEND_TOKEN => self.sys_task_suspend_token(a0 as _, a1.into()),
            SyscallType::PROCESS_START => {
                self.sys_process_start(a0 as _, a1 as _, a2 as _, a3 as _, a4 as _, a5 as _)
            }
            SyscallType::OBJECT_WAIT_ONE => {
                { self.sys_object_wait_one(a0 as _, a1 as _, a2 as _, a3.into()) }.await
            }
            SyscallType::THREAD_WRITE_STATE => {
                self.sys_thread_write_state(a0 as _, a1 as _, a2.into(), a3 as _)
            }
            SyscallType::OBJECT_GET_INFO => {
                self.sys_object_get_info(a0 as _, a1 as _, a2 as _, a3 as _, a4.into(), a5.into())
            }
            SyscallType::VMO_REPLACE_AS_EXECUTABLE => {
                self.sys_vmo_replace_as_executable(a0 as _, a1 as _, a2.into())
            }
            SyscallType::VMO_GET_SIZE => self.sys_vmo_get_size(a0 as _, a1.into()),
            SyscallType::CHANNEL_CREATE => self.sys_channel_create(a0 as _, a1.into(), a2.into()),
            SyscallType::VMO_CREATE_CHILD => {
                self.sys_vmo_create_child(a0 as _, a1 as _, a2 as _, a3 as _, a4.into())
            }
            SyscallType::HANDLE_REPLACE => self.sys_handle_replace(a0 as _, a1 as _, a2.into()),
            SyscallType::CHANNEL_WRITE => {
                self.sys_channel_write(a0 as _, a1 as _, a2.into(), a3 as _, a4.into(), a5 as _)
            }
            SyscallType::VMAR_DESTROY => self.sys_vmar_destroy(a0 as _),
            SyscallType::CHANNEL_CALL_NORETRY => {
                self.sys_channel_call_noretry(
                    a0 as _,
                    a1 as _,
                    a2 as _,
                    a3.into(),
                    a4.into(),
                    a5.into(),
                )
                .await
            }
            SyscallType::VMO_SET_SIZE => self.sys_vmo_set_size(a0 as _, a1 as _),
            SyscallType::VMAR_PROTECT => self.sys_vmar_protect(a0 as _, a1 as _, a2 as _, a3 as _),
            SyscallType::JOB_SET_CRITICAL => self.sys_job_set_critical(a0 as _, a1 as _, a2 as _),
            SyscallType::PORT_CREATE => self.sys_port_create(a0 as _, a1.into()),
            SyscallType::TIMER_CREATE => self.sys_timer_create(a0 as _, a1 as _, a2.into()),
            SyscallType::EVENT_CREATE => self.sys_event_create(a0 as _, a1.into()),
            SyscallType::CLOCK_GET => self.sys_clock_get(a0 as _, a1.into()),
            SyscallType::VMAR_UNMAP => self.sys_vmar_unmap(a0 as _, a1 as _, a2 as _),
            SyscallType::RESOURCE_CREATE => self.sys_resource_create(
                a0 as _,
                a1 as _,
                a2 as _,
                a3 as _,
                a4.into(),
                a5 as _,
                a6.into(),
            ),
            SyscallType::VMO_OP_RANGE => {
                self.sys_vmo_op_range(a0 as _, a1 as _, a2 as _, a3 as _, a4.into(), a5 as _)
            }
            SyscallType::THREAD_START => {
                self.sys_thread_start(a0 as _, a1 as _, a2 as _, a3 as _, a4 as _)
            }
            SyscallType::PORT_WAIT => self.sys_port_wait(a0 as _, a1 as _, a2.into()).await,
            SyscallType::OBJECT_SIGNAL_PEER => {
                self.sys_object_signal_peer(a0 as _, a1 as _, a2 as _)
            }
            SyscallType::OBJECT_WAIT_ASYNC => {
                self.sys_object_wait_async(a0 as _, a1 as _, a2 as _, a3 as _, a4 as _)
            }
            SyscallType::PORT_QUEUE => self.sys_port_queue(a0 as _, a1.into()),
            _ => {
                warn!("syscall unimplemented: {:?}", sys_type);
                Err(ZxError::NOT_SUPPORTED)
            }
        };
        info!("{} {:?} <= {:?}", thread_name, sys_type, ret);
        match ret {
            Ok(_) => 0,
            Err(err) => err as isize,
        }
    }
}
