#![no_std]
#![deny(unsafe_code, unused_must_use)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use crate::util::*;
use alloc::sync::Arc;
use alloc::vec::Vec;
use zircon_object::ipc::channel::Channel;
use zircon_object::object::*;
use zircon_object::task::thread::Thread;
use zircon_object::ZxResult;

mod util;

pub struct Syscall {
    thread: Arc<Thread>,
}

impl Syscall {
    /// Read a message from a channel.
    pub fn sys_channel_read(
        &self,
        handle_value: HandleValue,
        options: u32,
        bytes: UserOutPtr<u8>,
        handles: UserOutPtr<HandleValue>,
        num_bytes: u32,
        num_handles: u32,
        actual_bytes: UserOutPtr<u32>,
        actual_handles: UserOutPtr<u32>,
    ) -> ZxResult<()> {
        info!(
            "channel.read: handle={:?}, options={:?}, bytes={:?}, handles={:?}, num_bytes={:?}, num_handles={:?}",
            handle_value, options, bytes, handles, num_bytes, num_handles,
        );
        let proc = &self.thread.proc;
        let channel = proc.get_object_with_rights::<Channel>(handle_value, Rights::READ)?;
        let msg = channel.read()?;
        actual_bytes.write_if_not_null(msg.data.len() as u32)?;
        actual_handles.write_if_not_null(msg.handles.len() as u32)?;
        if num_bytes < msg.data.len() as u32 || num_handles < msg.handles.len() as u32 {
            const MAY_DISCARD: u32 = 1;
            if options & MAY_DISCARD == 0 {
                unimplemented!("always discard when buffer too small for now");
            }
            return Err(ZxError::BUFFER_TOO_SMALL);
        }
        bytes.write_array(msg.data.as_slice())?;
        let handle_values: Vec<_> = msg
            .handles
            .into_iter()
            .map(|handle| proc.add_handle(handle))
            .collect();
        handles.write_array(handle_values.as_slice())?;
        unimplemented!()
    }
}
