use {super::*, zircon_object::ipc::Channel};

impl Syscall {
    /// Read a message from a channel.
    #[allow(clippy::too_many_arguments)]
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
    ) -> ZxResult<usize> {
        info!(
            "channel.read: handle={:?}, options={:?}, bytes=({:?}; {:?}), handles=({:?}; {:?})",
            handle_value, options, bytes, num_bytes, handles, num_handles,
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
        Ok(ZxError::OK as usize)
    }
}
