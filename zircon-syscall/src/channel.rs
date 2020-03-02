use {
    super::*,
    zircon_object::ipc::{Channel, MessagePacket},
};

impl Syscall {
    /// Read a message from a channel.
    #[allow(clippy::too_many_arguments)]
    pub fn sys_channel_read(
        &self,
        handle_value: HandleValue,
        options: u32,
        mut bytes: UserOutPtr<u8>,
        mut handles: UserOutPtr<HandleValue>,
        num_bytes: u32,
        num_handles: u32,
        mut actual_bytes: UserOutPtr<u32>,
        mut actual_handles: UserOutPtr<u32>,
    ) -> ZxResult<usize> {
        info!(
            "channel.read: handle={:?}, options={:?}, bytes=({:?}; {:?}), handles=({:?}; {:?})",
            handle_value, options, bytes, num_bytes, handles, num_handles,
        );
        let proc = self.thread.proc();
        let channel = proc.get_object_with_rights::<Channel>(handle_value, Rights::READ)?;
        let msg = channel.read()?;
        actual_bytes.write_if_not_null(msg.data.len() as u32)?;
        actual_handles.write_if_not_null(msg.handles.len() as u32)?;
        if num_bytes < msg.data.len() as u32 || num_handles < msg.handles.len() as u32 {
            const MAY_DISCARD: u32 = 1;
            if options & MAY_DISCARD == 1 {
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
        Ok(ZxError::OK as usize)
    }

    pub fn sys_channel_write(
        &self,
        handle_value: HandleValue,
        options: u32,
        user_bytes: UserInPtr<u8>,
        num_bytes: u32,
        user_handles: UserInPtr<HandleValue>,
        num_handles: u32,
    ) -> ZxResult<usize> {
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        info!(
            "channel.write: handle_value={}, num_bytes={:#x}, num_handles={:#x}",
            handle_value, num_bytes, num_handles,
        );
        let proc = self.thread.proc();
        let mut handles = Vec::new();
        let user_handles = user_handles.read_array(num_handles as usize)?;
        for handle in user_handles {
            handles.push(proc.remove_handle(handle)?);
        }
        let channel = proc.get_object_with_rights::<Channel>(handle_value, Rights::WRITE)?;
        channel.write(MessagePacket {
            data: user_bytes.read_array(num_bytes as usize)?,
            handles,
        })?;
        Ok(0)
    }

    pub fn sys_channel_create(
        &self,
        options: u32,
        mut out0: UserOutPtr<HandleValue>,
        mut out1: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        if options != 0u32 {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let (end0, end1) = Channel::create();
        let handle0 = proc.add_handle(Handle::new(end0, Rights::DEFAULT_CHANNEL));
        let handle1 = proc.add_handle(Handle::new(end1, Rights::DEFAULT_CHANNEL));
        out0.write(handle0)?;
        out1.write(handle1)?;
        Ok(0)
    }
}
