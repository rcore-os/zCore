use {
    super::*,
    alloc::vec::Vec,
    kernel_hal::MMUFlags,
    zircon_object::{
        ipc::{Channel, MessagePacket},
        object::{obj_type, HandleInfo},
        task::{Process, ThreadState},
    },
};

impl Syscall<'_> {
    #[allow(clippy::too_many_arguments)]
    /// Read/Receive a message from a channel.
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
        if options & !MAY_DISCARD != 0 {
            return Err(ZxError::NOT_SUPPORTED);
        }
        let never_discard = options & MAY_DISCARD == 0;

        let msg = if never_discard {
            channel.check_and_read(|front_msg| {
                validate_optional_user_range(
                    proc,
                    actual_bytes.as_addr(),
                    core::mem::size_of::<u32>(),
                    MMUFlags::WRITE,
                )?;
                validate_optional_user_range(
                    proc,
                    actual_handles.as_addr(),
                    core::mem::size_of::<u32>(),
                    MMUFlags::WRITE,
                )?;
                if num_bytes < front_msg.data.len() as u32
                    || num_handles < front_msg.handles.len() as u32
                {
                    actual_bytes.write_if_not_null(front_msg.data.len() as u32)?;
                    actual_handles.write_if_not_null(front_msg.handles.len() as u32)?;
                    Err(ZxError::BUFFER_TOO_SMALL)
                } else {
                    validate_user_range(
                        proc,
                        bytes.as_addr(),
                        front_msg.data.len(),
                        MMUFlags::WRITE,
                    )?;
                    let handle_size = if is_etc {
                        core::mem::size_of::<HandleInfo>()
                    } else {
                        core::mem::size_of::<HandleValue>()
                    };
                    validate_user_range(
                        proc,
                        handles,
                        front_msg
                            .handles
                            .len()
                            .checked_mul(handle_size)
                            .ok_or(ZxError::INVALID_ARGS)?,
                        MMUFlags::WRITE,
                    )?;
                    Ok(())
                }
            })?
        } else {
            channel.read()?
        };

        validate_optional_user_range(
            proc,
            actual_bytes.as_addr(),
            core::mem::size_of::<u32>(),
            MMUFlags::WRITE,
        )?;
        validate_optional_user_range(
            proc,
            actual_handles.as_addr(),
            core::mem::size_of::<u32>(),
            MMUFlags::WRITE,
        )?;
        actual_bytes.write_if_not_null(msg.data.len() as u32)?;
        actual_handles.write_if_not_null(msg.handles.len() as u32)?;
        if num_bytes < msg.data.len() as u32 || num_handles < msg.handles.len() as u32 {
            return Err(ZxError::BUFFER_TOO_SMALL);
        }
        validate_user_range(proc, bytes.as_addr(), msg.data.len(), MMUFlags::WRITE)?;
        let handle_size = if is_etc {
            core::mem::size_of::<HandleInfo>()
        } else {
            core::mem::size_of::<HandleValue>()
        };
        validate_user_range(
            proc,
            handles,
            msg.handles
                .len()
                .checked_mul(handle_size)
                .ok_or(ZxError::INVALID_ARGS)?,
            MMUFlags::WRITE,
        )?;
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
    /// Write a message to a channel.
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
        const USE_IOVEC: u32 = 2;
        if options & !USE_IOVEC != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let data = if options & USE_IOVEC != 0 {
            read_channel_iovecs(proc, user_bytes, num_bytes)?
        } else {
            if num_bytes > 65536 {
                return Err(ZxError::OUT_OF_RANGE);
            }
            validate_user_range(
                proc,
                user_bytes.as_addr(),
                num_bytes as usize,
                MMUFlags::READ,
            )?;
            user_bytes.read_array(num_bytes as usize)?
        };
        validate_user_range(
            proc,
            user_handles.as_addr(),
            (num_handles as usize)
                .checked_mul(core::mem::size_of::<HandleValue>())
                .ok_or(ZxError::INVALID_ARGS)?,
            MMUFlags::READ,
        )?;
        let handles = user_handles.as_slice(num_handles as usize)?;
        let transfer_self = handles.contains(&handle_value);
        let handles = proc.remove_handles(handles)?;
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
    /// Create a new channel.
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
        deadline: Deadline,
        user_args: UserInPtr<ChannelCallArgs>,
        mut actual_bytes: UserOutPtr<u32>,
        mut actual_handles: UserOutPtr<u32>,
    ) -> ZxResult {
        let proc = self.thread.proc();
        validate_user_range(
            proc,
            user_args.as_addr(),
            core::mem::size_of::<ChannelCallArgs>(),
            MMUFlags::READ,
        )?;
        let mut args = user_args.read()?;
        info!(
            "channel.call_noretry: handle={:#x}, deadline={:?}, args={:#x?}",
            handle_value, deadline, args
        );
        const USE_IOVEC: u32 = 2;
        if options & !USE_IOVEC != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let data = if options & USE_IOVEC != 0 {
            read_channel_iovecs(proc, args.wr_bytes, args.wr_num_bytes)?
        } else {
            validate_user_range(
                proc,
                args.wr_bytes.as_addr(),
                args.wr_num_bytes as usize,
                MMUFlags::READ,
            )?;
            args.wr_bytes.read_array(args.wr_num_bytes as usize)?
        };
        if args.rd_num_bytes < 4 || data.len() < 4 {
            return Err(ZxError::INVALID_ARGS);
        }
        validate_user_range(
            proc,
            args.wr_handles.as_addr(),
            (args.wr_num_handles as usize)
                .checked_mul(core::mem::size_of::<HandleValue>())
                .ok_or(ZxError::INVALID_ARGS)?,
            MMUFlags::READ,
        )?;
        let (channel, channel_rights) = proc.get_object_and_rights::<Channel>(handle_value)?;
        if !channel_rights.contains(Rights::READ | Rights::WRITE) {
            return Err(ZxError::ACCESS_DENIED);
        }
        let wr_msg = MessagePacket {
            data,
            handles: {
                let handles = args.wr_handles.as_slice(args.wr_num_handles as usize)?;
                let handles = proc.remove_handles(handles)?;
                for handle in handles.iter() {
                    if !handle.rights.contains(Rights::TRANSFER) {
                        return Err(ZxError::ACCESS_DENIED);
                    }
                }
                handles
            },
        };

        let future = channel.call(wr_msg);
        pin_mut!(future);
        let rd_msg: MessagePacket = self
            .thread
            .blocking_run(future, ThreadState::BlockedChannel, deadline.into(), None)
            .await?;

        // A channel call must send its request even when one of the receive-side
        // pointers is invalid. Validate them after the peer has replied, but
        // before copying the reply or installing any returned handles.
        validate_user_range(
            proc,
            args.rd_bytes.as_addr(),
            args.rd_num_bytes as usize,
            MMUFlags::WRITE,
        )?;
        validate_user_range(
            proc,
            args.rd_handles.as_addr(),
            (args.rd_num_handles as usize)
                .checked_mul(core::mem::size_of::<HandleValue>())
                .ok_or(ZxError::INVALID_ARGS)?,
            MMUFlags::WRITE,
        )?;
        validate_user_range(
            proc,
            actual_bytes.as_addr(),
            core::mem::size_of::<u32>(),
            MMUFlags::WRITE,
        )?;
        validate_user_range(
            proc,
            actual_handles.as_addr(),
            core::mem::size_of::<u32>(),
            MMUFlags::WRITE,
        )?;
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

    #[allow(clippy::too_many_arguments)]
    pub async fn sys_channel_call_etc_noretry(
        &self,
        handle_value: HandleValue,
        options: u32,
        deadline: Deadline,
        user_args: UserInPtr<ChannelCallEtcArgs>,
        mut actual_bytes: UserOutPtr<u32>,
        mut actual_handles: UserOutPtr<u32>,
    ) -> ZxResult {
        const USE_IOVEC: u32 = 2;
        if options & !USE_IOVEC != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        validate_user_range(
            proc,
            user_args.as_addr(),
            core::mem::size_of::<ChannelCallEtcArgs>(),
            MMUFlags::READ,
        )?;
        let mut args = user_args.read()?;
        let data = if options & USE_IOVEC != 0 {
            read_channel_iovecs(proc, args.wr_bytes, args.wr_num_bytes)?
        } else {
            validate_user_range(
                proc,
                args.wr_bytes.as_addr(),
                args.wr_num_bytes as usize,
                MMUFlags::READ,
            )?;
            args.wr_bytes.read_array(args.wr_num_bytes as usize)?
        };
        if args.rd_num_bytes < 4 || data.len() < 4 {
            return Err(ZxError::INVALID_ARGS);
        }
        validate_user_range(
            proc,
            args.wr_handles.as_addr(),
            (args.wr_num_handles as usize)
                .checked_mul(core::mem::size_of::<HandleDisposition>())
                .ok_or(ZxError::INVALID_ARGS)?,
            MMUFlags::READ | MMUFlags::WRITE,
        )?;
        let (channel, channel_rights) = proc.get_object_and_rights::<Channel>(handle_value)?;
        if !channel_rights.contains(Rights::READ | Rights::WRITE) {
            return Err(ZxError::ACCESS_DENIED);
        }
        let mut dispositions = args.wr_handles.read_array(args.wr_num_handles as usize)?;
        let mut handles = Vec::with_capacity(dispositions.len());
        let mut first_error = None;
        for disposition in &mut dispositions {
            match proc.get_dyn_object_and_rights(disposition.handle) {
                Ok((object, src_rights)) => {
                    if let Err(error) = handle_check(disposition, &object, src_rights, handle_value)
                    {
                        disposition.result = error as i32;
                        first_error.get_or_insert(error);
                        continue;
                    }
                    let rights = if disposition.rights == Rights::SAME_RIGHTS.bits() {
                        src_rights
                    } else {
                        Rights::from_bits(disposition.rights).ok_or(ZxError::INVALID_ARGS)?
                    };
                    handles.push(Handle::new(object, rights));
                    if disposition.op == ZX_HANDLE_OP_MOVE {
                        proc.remove_handle(disposition.handle)?;
                    }
                }
                Err(error) => {
                    disposition.result = error as i32;
                    first_error.get_or_insert(error);
                }
            }
        }
        args.wr_handles.write_array(&dispositions)?;
        if let Some(error) = first_error {
            return Err(error);
        }

        let future = channel.call(MessagePacket { data, handles });
        pin_mut!(future);
        let rd_msg: MessagePacket = self
            .thread
            .blocking_run(future, ThreadState::BlockedChannel, deadline.into(), None)
            .await?;
        // A channel call must send its request even when one of the receive-side
        // pointers is invalid. Validate them after the peer has replied, but
        // before consuming the reply or installing any returned handles.
        validate_user_range(
            proc,
            args.rd_bytes.as_addr(),
            args.rd_num_bytes as usize,
            MMUFlags::WRITE,
        )?;
        validate_user_range(
            proc,
            args.rd_handles.as_addr(),
            (args.rd_num_handles as usize)
                .checked_mul(core::mem::size_of::<HandleInfo>())
                .ok_or(ZxError::INVALID_ARGS)?,
            MMUFlags::WRITE,
        )?;
        validate_user_range(
            proc,
            actual_bytes.as_addr(),
            core::mem::size_of::<u32>(),
            MMUFlags::WRITE,
        )?;
        validate_user_range(
            proc,
            actual_handles.as_addr(),
            core::mem::size_of::<u32>(),
            MMUFlags::WRITE,
        )?;
        actual_bytes.write(rd_msg.data.len() as u32)?;
        actual_handles.write(rd_msg.handles.len() as u32)?;
        if args.rd_num_bytes < rd_msg.data.len() as u32
            || args.rd_num_handles < rd_msg.handles.len() as u32
        {
            return Err(ZxError::BUFFER_TOO_SMALL);
        }
        args.rd_bytes.write_array(&rd_msg.data)?;
        let mut infos: Vec<HandleInfo> =
            rd_msg.handles.iter().map(Handle::get_handle_info).collect();
        let values = proc.add_handles(rd_msg.handles);
        for (info, value) in infos.iter_mut().zip(values) {
            info.handle = value;
        }
        args.rd_handles.write_array(&infos)?;
        Ok(())
    }

    pub fn sys_channel_call_finish(
        &self,
        deadline: Deadline,
        user_args: UserInPtr<ChannelCallArgs>,
        _actual_bytes: UserOutPtr<u32>,
        _actual_handles: UserOutPtr<u32>,
    ) -> ZxResult {
        let args = user_args.read()?;
        info!(
            "channel.call_finish: deadline={:?}, args={:#x?}",
            deadline, args
        );
        let thread_state = self.thread.state();
        if thread_state == ThreadState::BlockedChannel {
            unimplemented!();
        } else {
            Err(ZxError::BAD_STATE)
        }
    }
    /// Write a message to a channel.
    pub fn sys_channel_write_etc(
        &self,
        handle: HandleValue,
        options: u32,
        user_bytes: UserInPtr<u8>,
        num_bytes: u32,
        mut user_handles: UserInOutPtr<HandleDisposition>,
        num_handles: u32,
    ) -> ZxResult {
        info!(
            "channel.write_etc: handle={:#x}, options={:#x}, user_bytes={:#x?}, num_bytes={:#x}, user_handles={:#x?}, num_handles={:#x}",
            handle, options, user_bytes, num_bytes, user_handles, num_handles
        );
        const USE_IOVEC: u32 = 2;
        let invalid_options = options & !USE_IOVEC != 0;
        let proc = self.thread.proc();
        let data = if options & USE_IOVEC != 0 {
            read_channel_iovecs(proc, user_bytes, num_bytes)?
        } else {
            if num_bytes > 65536 {
                return Err(ZxError::OUT_OF_RANGE);
            }
            validate_user_range(
                proc,
                user_bytes.as_addr(),
                num_bytes as usize,
                MMUFlags::READ,
            )?;
            user_bytes.read_array(num_bytes as usize)?
        };
        validate_user_range(
            proc,
            user_handles.as_addr(),
            (num_handles as usize)
                .checked_mul(core::mem::size_of::<HandleDisposition>())
                .ok_or(ZxError::INVALID_ARGS)?,
            MMUFlags::READ | MMUFlags::WRITE,
        )?;
        let mut dispositions = user_handles.read_array(num_handles as usize)?;
        let mut handles: Vec<Handle> = Vec::new();
        let mut ret: ZxResult = Ok(());
        for disposition in dispositions.iter_mut() {
            if let Ok((object, src_rights)) = proc.get_dyn_object_and_rights(disposition.handle) {
                if let Err(e) = handle_check(disposition, &object, src_rights, handle) {
                    disposition.result = e as _;
                    if ret.is_ok() {
                        ret = Err(e);
                    }
                }
                let new_rights = if disposition.rights != Rights::SAME_RIGHTS.bits() {
                    Rights::from_bits(disposition.rights).unwrap()
                } else {
                    src_rights
                };
                let new_handle = Handle::new(object, new_rights);
                if disposition.op != ZX_HANDLE_OP_DUP {
                    proc.remove_handle(disposition.handle).unwrap();
                }
                handles.push(new_handle);
            } else {
                disposition.result = ZxError::BAD_HANDLE as _;
                if ret.is_ok() {
                    ret = Err(ZxError::BAD_HANDLE);
                }
            }
        }
        user_handles.write_array(&dispositions)?;
        if invalid_options {
            return Err(ZxError::INVALID_ARGS);
        }
        if num_handles > 64 {
            return Err(ZxError::OUT_OF_RANGE);
        }
        ret?;
        let channel = proc.get_object_with_rights::<Channel>(handle, Rights::WRITE)?;
        channel.write(MessagePacket { data, handles })?;
        Ok(())
    }
}

fn handle_check(
    disposition: &HandleDisposition,
    object: &Arc<dyn KernelObject>,
    src_rights: Rights,
    handle_value: HandleValue,
) -> ZxResult {
    if !src_rights.contains(Rights::TRANSFER) {
        Err(ZxError::ACCESS_DENIED)
    } else if disposition.handle == handle_value {
        Err(ZxError::NOT_SUPPORTED)
    } else if disposition.type_ != 0 && disposition.type_ != obj_type(object) {
        Err(ZxError::WRONG_TYPE)
    } else if disposition.op != ZX_HANDLE_OP_MOVE && disposition.op != ZX_HANDLE_OP_DUP
        || disposition.rights != Rights::SAME_RIGHTS.bits()
            && (!src_rights.bits() & disposition.rights) != 0
    {
        Err(ZxError::INVALID_ARGS)
    } else if disposition.op == ZX_HANDLE_OP_DUP && !src_rights.contains(Rights::DUPLICATE) {
        Err(ZxError::ACCESS_DENIED)
    } else {
        Ok(())
    }
}

const ZX_HANDLE_OP_MOVE: u32 = 0;
const ZX_HANDLE_OP_DUP: u32 = 1;

#[repr(C)]
struct ChannelIoVec {
    buffer: UserInPtr<u8>,
    capacity: u32,
    reserved: u32,
}

fn read_channel_iovecs(proc: &Process, ptr: UserInPtr<u8>, count: u32) -> ZxResult<Vec<u8>> {
    const MAX_IOVECS: u32 = 8192;
    const MAX_MESSAGE_BYTES: usize = 65536;
    if count > MAX_IOVECS {
        return Err(ZxError::OUT_OF_RANGE);
    }
    validate_user_range(
        proc,
        ptr.as_addr(),
        (count as usize)
            .checked_mul(core::mem::size_of::<ChannelIoVec>())
            .ok_or(ZxError::INVALID_ARGS)?,
        MMUFlags::READ,
    )?;
    let iovecs = UserInPtr::<ChannelIoVec>::from(ptr.as_addr()).read_array(count as usize)?;
    let mut data = Vec::new();
    for iovec in iovecs {
        if iovec.reserved != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let new_len = data
            .len()
            .checked_add(iovec.capacity as usize)
            .ok_or(ZxError::OUT_OF_RANGE)?;
        if new_len > MAX_MESSAGE_BYTES {
            return Err(ZxError::OUT_OF_RANGE);
        }
        validate_user_range(
            proc,
            iovec.buffer.as_addr(),
            iovec.capacity as usize,
            MMUFlags::READ,
        )?;
        data.extend_from_slice(iovec.buffer.as_slice(iovec.capacity as usize)?);
    }
    Ok(data)
}

pub(crate) fn validate_user_range(
    proc: &Process,
    addr: usize,
    len: usize,
    access: MMUFlags,
) -> ZxResult {
    proc.vmar()
        .check_user_range(addr, len, access)
        .map_err(|_| ZxError::INVALID_ARGS)
}

fn validate_optional_user_range(
    proc: &Process,
    addr: usize,
    len: usize,
    access: MMUFlags,
) -> ZxResult {
    if addr == 0 {
        Ok(())
    } else {
        validate_user_range(proc, addr, len, access)
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

#[repr(C)]
#[derive(Debug)]
pub struct ChannelCallEtcArgs {
    wr_bytes: UserInPtr<u8>,
    wr_handles: UserInOutPtr<HandleDisposition>,
    rd_bytes: UserOutPtr<u8>,
    rd_handles: UserOutPtr<HandleInfo>,
    wr_num_bytes: u32,
    wr_num_handles: u32,
    rd_num_bytes: u32,
    rd_num_handles: u32,
}

#[repr(C)]
#[derive(Debug)]
pub struct HandleDisposition {
    op: u32,
    handle: HandleValue,
    type_: u32,
    rights: u32,
    result: i32,
}
