use {
    super::*,
    zircon_object::ipc::{Channel, MessagePacket},
};

impl Syscall<'_> {
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
            "channel.read: handle={:#x?}, options={:?}, bytes=({:#x?}; {:#x?}), handles=({:#x?}; {:#x?})",
            handle_value, options, bytes, num_bytes, handles, num_handles,
        );
        let proc = self.thread.proc();
        let channel = proc.get_object_with_rights::<Channel>(handle_value, Rights::READ)?;
        const MAY_DISCARD: u32 = 1;
        let never_discard = options & MAY_DISCARD == 0;
        let msg = if never_discard {
            channel.check_and_read(|front_msg| {
                if num_bytes < front_msg.data.len() as u32
                    || num_handles < front_msg.handles.len() as u32
                {
                    actual_bytes.write_if_not_null(front_msg.data.len() as u32)?;
                    actual_handles.write_if_not_null(front_msg.handles.len() as u32)?;
                    Err(ZxError::BUFFER_TOO_SMALL)
                } else {
                    Ok(())
                }
            })?
        } else {
            channel.read()?
        };
        actual_bytes.write_if_not_null(msg.data.len() as u32)?;
        actual_handles.write_if_not_null(msg.handles.len() as u32)?;
        bytes.write_array(msg.data.as_slice())?;
        let handle_values: Vec<_> = msg
            .handles
            .into_iter()
            .map(|handle| proc.add_handle(handle))
            .collect();
        handles.write_array(handle_values.as_slice())?;
        Ok(0)
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
            "channel.write: handle_value={:#x}, num_bytes={:#x}, num_handles={:#x}",
            handle_value, num_bytes, num_handles,
        );
        let proc = self.thread.proc();
        let mut handles = Vec::new();
        let user_handles = user_handles.read_array(num_handles as usize)?;
        info!("handles: {:?}", user_handles);
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
        info!("channel.create: options={:#x}", options);
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

    pub async fn sys_channel_call_noretry(
        &self,
        handle_value: HandleValue,
        options: u32,
        _deadline: u64,
        user_args: UserInPtr<ChannelCallArgs>,
        mut actual_bytes: UserOutPtr<u32>,
        mut actual_handles: UserOutPtr<u32>,
    ) -> ZxResult<usize> {
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut args = user_args.read()?;
        info!(
            "channel.call_noretry: handle={:#x}, args={:#x?}",
            handle_value, args
        );
        let proc = self.thread.proc();
        let kobject =
            proc.get_dyn_object_with_rights(handle_value, Rights::READ | Rights::WRITE)?;
        let channel = proc.get_object::<Channel>(handle_value)?;
        let wr_msg = MessagePacket {
            data: args.wr_bytes.read_array(args.wr_num_bytes as usize)?,
            handles: {
                let mut res = Vec::new();
                let handles = args.wr_handles.read_array(args.wr_num_handles as usize)?;
                for handle in handles {
                    res.push(proc.remove_handle(handle)?);
                }
                res
            },
        };
        channel.write(wr_msg)?;
        kobject.wait_signal(Signal::READABLE).await;
        let recv_msg = channel.read()?;
        actual_bytes.write_if_not_null(recv_msg.data.len() as u32)?;
        actual_handles.write_if_not_null(recv_msg.handles.len() as u32)?;
        args.rd_bytes.write_array(recv_msg.data.as_slice())?;
        let handles: Vec<_> = recv_msg
            .handles
            .into_iter()
            .map(|handle| proc.add_handle(handle))
            .collect();
        args.rd_handles.write_array(handles.as_slice())?;
        Ok(0)
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct ChannelCallArgs {
    wr_bytes: UserInPtr<u8>,
    wr_handles: UserInPtr<HandleValue>,
    rd_bytes: UserOutPtr<u8>,
    rd_handles: UserOutPtr<HandleValue>,
    wr_num_bytes: u32,
    wr_num_handles: u32,
    rd_num_bytes: u32,
    rd_num_handles: u32,
}
