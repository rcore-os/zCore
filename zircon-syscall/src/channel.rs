use {
    super::*,
    alloc::vec::Vec,
    zircon_object::{
        ipc::{Channel, MessagePacket},
        object::HandleInfo,
        task::ThreadState,
    },
};

impl Syscall<'_> {
    /// Read a message from a channel.
    #[allow(clippy::too_many_arguments)]
    pub fn sys_channel_read(
        &self,
        handle_value: HandleValue,
        options: u32,
        mut bytes: UserOutPtr<u8>,
        handles: usize,
        num_bytes: u32,
        num_handles: u32,
        mut actual_bytes: UserOutPtr<u32>,
        mut actual_handles: UserOutPtr<u32>,
        is_etc: bool,
    ) -> ZxResult {
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
        if num_bytes < msg.data.len() as u32 || num_handles < msg.handles.len() as u32 {
            return Err(ZxError::BUFFER_TOO_SMALL);
        }
        bytes.write_array(msg.data.as_slice())?;
        if is_etc {
            let mut handle_infos: Vec<HandleInfo> = msg
                .handles
                .iter()
                .map(|handle| handle.get_handle_info())
                .collect();
            let values = proc.add_handles(msg.handles);
            for (i, value) in values.iter().enumerate() {
                handle_infos[i].handle = *value;
            }
            UserOutPtr::<HandleInfo>::from(handles).write_array(&handle_infos)?;
        } else {
            let values = proc.add_handles(msg.handles);
            UserOutPtr::<HandleValue>::from(handles).write_array(&values)?;
        }
        Ok(())
    }

    pub fn sys_channel_write(
        &self,
        handle_value: HandleValue,
        options: u32,
        user_bytes: UserInPtr<u8>,
        num_bytes: u32,
        user_handles: UserInPtr<HandleValue>,
        num_handles: u32,
    ) -> ZxResult {
        info!(
            "channel.write: handle_value={:#x}, num_bytes={:#x}, num_handles={:#x}",
            handle_value, num_bytes, num_handles,
        );
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        if num_bytes > 65536 {
            return Err(ZxError::OUT_OF_RANGE);
        }
        let proc = self.thread.proc();
        let data = user_bytes.read_array(num_bytes as usize)?;
        let handles = user_handles.read_array(num_handles as usize)?;
        let transfer_self = handles.iter().any(|&handle| handle == handle_value);
        let handles = proc.remove_handles(&handles)?;
        if transfer_self {
            return Err(ZxError::NOT_SUPPORTED);
        }
        if handles.len() > 64 {
            return Err(ZxError::OUT_OF_RANGE);
        }
        for handle in handles.iter() {
            if !handle.rights.contains(Rights::TRANSFER) {
                return Err(ZxError::ACCESS_DENIED);
            }
        }
        let channel = proc.get_object_with_rights::<Channel>(handle_value, Rights::WRITE)?;
        channel.write(MessagePacket { data, handles })?;
        Ok(())
    }

    pub fn sys_channel_create(
        &self,
        options: u32,
        mut out0: UserOutPtr<HandleValue>,
        mut out1: UserOutPtr<HandleValue>,
    ) -> ZxResult {
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
        Ok(())
    }

    pub async fn sys_channel_call_noretry(
        &self,
        handle_value: HandleValue,
        options: u32,
        deadline: i64,
        user_args: UserInPtr<ChannelCallArgs>,
        mut actual_bytes: UserOutPtr<u32>,
        mut actual_handles: UserOutPtr<u32>,
    ) -> ZxResult {
        let mut args = user_args.read()?;
        info!(
            "channel.call_noretry: handle={:#x}, deadline={:#x}, args={:#x?}",
            handle_value, deadline, args
        );
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        if args.rd_num_bytes < 4 || args.wr_num_bytes < 4 {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let channel =
            proc.get_object_with_rights::<Channel>(handle_value, Rights::READ | Rights::WRITE)?;
        let wr_msg = MessagePacket {
            data: args.wr_bytes.read_array(args.wr_num_bytes as usize)?,
            handles: {
                let handles = args.wr_handles.read_array(args.wr_num_handles as usize)?;
                let handles = proc.remove_handles(&handles)?;
                for handle in handles.iter() {
                    if !handle.rights.contains(Rights::TRANSFER) {
                        return Err(ZxError::ACCESS_DENIED);
                    }
                }
                handles
            },
        };

        if deadline <= 0 {
            channel.write(wr_msg)?;
            return Err(ZxError::TIMED_OUT);
        }
        if deadline != i64::max_value() {
            unimplemented!()
        }

        let future = channel.call(wr_msg);
        let rd_msg = self
            .thread
            .blocking_run(future, ThreadState::BlockedChannel)
            .await?;

        actual_bytes.write(rd_msg.data.len() as u32)?;
        actual_handles.write(rd_msg.handles.len() as u32)?;
        if args.rd_num_bytes < rd_msg.data.len() as u32
            || args.rd_num_handles < rd_msg.handles.len() as u32
        {
            return Err(ZxError::BUFFER_TOO_SMALL);
        }
        args.rd_bytes.write_array(rd_msg.data.as_slice())?;
        args.rd_handles
            .write_array(&proc.add_handles(rd_msg.handles))?;
        Ok(())
    }

    pub fn sys_channel_call_finish(
        &self,
        deadline: i64,
        user_args: UserInPtr<ChannelCallArgs>,
        _actual_bytes: UserOutPtr<u32>,
        _actual_handles: UserOutPtr<u32>,
    ) -> ZxResult {
        let args = user_args.read()?;
        info!(
            "channel.call_finish: deadline={:#x}, args={:#x?}",
            deadline, args
        );
        let thread_state = self.thread.get_state();
        if thread_state == ThreadState::BlockedChannel {
            unimplemented!();
        } else {
            Err(ZxError::BAD_STATE)
        }
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
