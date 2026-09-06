use crate::user_memory::{validate_optional_user_range, validate_user_range};
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
        write_received_handles(proc, handles, msg.handles, is_etc)?;
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
        let proc = self.thread.proc();
        let (channel, message) = prepare_message(
            proc,
            handle_value,
            options,
            user_bytes,
            num_bytes,
            HandleBuffer::Values(user_handles.as_addr()),
            num_handles,
            false,
        )?;
        channel.write(message)
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
        actual_bytes: UserOutPtr<u32>,
        actual_handles: UserOutPtr<u32>,
    ) -> ZxResult {
        let proc = self.thread.proc();
        validate_user_range(
            proc,
            user_args.as_addr(),
            core::mem::size_of::<ChannelCallArgs>(),
            MMUFlags::READ,
        )?;
        let args = user_args.read()?;
        let (channel, message) = prepare_message(
            proc,
            handle_value,
            options,
            args.wr_bytes,
            args.wr_num_bytes,
            HandleBuffer::Values(args.wr_handles.as_addr()),
            args.wr_num_handles,
            true,
        )?;
        let future = channel.call(message);
        pin_mut!(future);
        let reply = self
            .thread
            .blocking_run(future, ThreadState::BlockedChannel, deadline.into(), None)
            .await?;
        // Receive pointers are checked after sending the request and awaiting
        // its reply, as required by channel-call semantics.
        ReplyBuffer {
            bytes: args.rd_bytes,
            handles: args.rd_handles.as_addr(),
            num_bytes: args.rd_num_bytes,
            num_handles: args.rd_num_handles,
            is_etc: false,
        }
        .write(proc, reply, actual_bytes, actual_handles)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn sys_channel_call_etc_noretry(
        &self,
        handle_value: HandleValue,
        options: u32,
        deadline: Deadline,
        user_args: UserInPtr<ChannelCallEtcArgs>,
        actual_bytes: UserOutPtr<u32>,
        actual_handles: UserOutPtr<u32>,
    ) -> ZxResult {
        let proc = self.thread.proc();
        validate_user_range(
            proc,
            user_args.as_addr(),
            core::mem::size_of::<ChannelCallEtcArgs>(),
            MMUFlags::READ,
        )?;
        let args = user_args.read()?;
        let (channel, message) = prepare_message(
            proc,
            handle_value,
            options,
            args.wr_bytes,
            args.wr_num_bytes,
            HandleBuffer::Dispositions(args.wr_handles.as_addr()),
            args.wr_num_handles,
            true,
        )?;
        let future = channel.call(message);
        pin_mut!(future);
        let reply = self
            .thread
            .blocking_run(future, ThreadState::BlockedChannel, deadline.into(), None)
            .await?;
        // Receive pointers are checked after sending the request and awaiting
        // its reply, as required by channel-call semantics.
        ReplyBuffer {
            bytes: args.rd_bytes,
            handles: args.rd_handles.as_addr(),
            num_bytes: args.rd_num_bytes,
            num_handles: args.rd_num_handles,
            is_etc: true,
        }
        .write(proc, reply, actual_bytes, actual_handles)
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
        user_handles: UserInOutPtr<HandleDisposition>,
        num_handles: u32,
    ) -> ZxResult {
        let proc = self.thread.proc();
        let (channel, message) = prepare_message(
            proc,
            handle,
            options,
            user_bytes,
            num_bytes,
            HandleBuffer::Dispositions(user_handles.as_addr()),
            num_handles,
            false,
        )?;
        channel.write(message)
    }
}

const USE_IOVEC: u32 = 2;
const MAX_MESSAGE_BYTES: usize = 65536;
const MAX_MESSAGE_HANDLES: u32 = 64;

/// Both send ABIs use the same transfer pipeline. Normalize ordinary handle
/// values to MOVE dispositions so validation and failure cleanup cannot drift.
#[derive(Clone, Copy)]
enum HandleBuffer {
    Values(usize),
    Dispositions(usize),
}

impl HandleBuffer {
    fn read_chunk(
        self,
        proc: &Process,
        offset: usize,
        count: usize,
    ) -> ZxResult<Vec<HandleDisposition>> {
        let (addr, size) = match self {
            Self::Values(addr) => (addr, core::mem::size_of::<HandleValue>()),
            Self::Dispositions(addr) => (addr, core::mem::size_of::<HandleDisposition>()),
        };
        let addr = offset
            .checked_mul(size)
            .and_then(|offset| addr.checked_add(offset))
            .ok_or(ZxError::INVALID_ARGS)?;
        validate_user_range(proc, addr, count * size, MMUFlags::READ)?;
        match self {
            Self::Values(_) => Ok(UserInPtr::<HandleValue>::from(addr)
                .read_array(count)?
                .into_iter()
                .map(|handle| HandleDisposition {
                    op: ZX_HANDLE_OP_MOVE,
                    handle,
                    type_: 0,
                    rights: Rights::SAME_RIGHTS.bits(),
                    result: 0,
                })
                .collect()),
            Self::Dispositions(_) => {
                Ok(UserInPtr::<HandleDisposition>::from(addr).read_array(count)?)
            }
        }
    }

    fn take(self, proc: &Process, count: u32) -> ZxResult<TakenHandles> {
        if count > MAX_MESSAGE_HANDLES {
            // Reject without allocating a user-sized array. The handle-release
            // ABI still requires closing MOVE handles on an oversized request.
            for offset in (0..count as usize).step_by(MAX_MESSAGE_HANDLES as usize) {
                let len = (count as usize - offset).min(MAX_MESSAGE_HANDLES as usize);
                let Ok(chunk) = self.read_chunk(proc, offset, len) else {
                    break;
                };
                for disposition in chunk {
                    if disposition.op != ZX_HANDLE_OP_DUP {
                        let _ = proc.remove_handle(disposition.handle);
                    }
                }
            }
            return Err(ZxError::OUT_OF_RANGE);
        }
        let dispositions = self.read_chunk(proc, 0, count as usize)?;
        let handles = dispositions
            .iter()
            .map(|disposition| {
                if disposition.op == ZX_HANDLE_OP_DUP {
                    proc.get_dyn_object_and_rights(disposition.handle)
                        .map(|(object, rights)| Handle::new(object, rights))
                } else {
                    // Take ownership before validation. Dropping the batch closes
                    // every MOVE handle on any subsequent error, including errors
                    // in another disposition, options, bytes or the channel itself.
                    proc.remove_handle(disposition.handle)
                }
            })
            .collect();
        Ok(TakenHandles {
            buffer: self,
            dispositions,
            handles,
        })
    }
}

struct TakenHandles {
    buffer: HandleBuffer,
    dispositions: Vec<HandleDisposition>,
    handles: Vec<ZxResult<Handle>>,
}

impl TakenHandles {
    fn finish(mut self, proc: &Process, channel: HandleValue) -> ZxResult<Vec<Handle>> {
        let mut first_error = None;
        let mut handles = Vec::with_capacity(self.handles.len());
        for (disposition, handle) in self.dispositions.iter_mut().zip(self.handles) {
            let result = handle.and_then(|mut handle| {
                handle_check(disposition, &handle.object, handle.rights, channel)?;
                if disposition.rights != Rights::SAME_RIGHTS.bits() {
                    handle.rights =
                        Rights::from_bits(disposition.rights).ok_or(ZxError::INVALID_ARGS)?;
                }
                Ok(handle)
            });
            match result {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    disposition.result = error as i32;
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first_error {
            if let HandleBuffer::Dispositions(addr) = self.buffer {
                validate_user_range(
                    proc,
                    addr,
                    self.dispositions.len() * core::mem::size_of::<HandleDisposition>(),
                    MMUFlags::WRITE,
                )?;
                UserOutPtr::<HandleDisposition>::from(addr).write_array(&self.dispositions)?;
            }
            return Err(error);
        }
        Ok(handles)
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_message(
    proc: &Process,
    channel: HandleValue,
    options: u32,
    bytes: UserInPtr<u8>,
    num_bytes: u32,
    handle_buffer: HandleBuffer,
    num_handles: u32,
    is_call: bool,
) -> ZxResult<(Arc<Channel>, MessagePacket)> {
    let rights = if is_call {
        Rights::READ | Rights::WRITE
    } else {
        Rights::WRITE
    };
    // Resolve the channel before consuming a possible self-transfer handle.
    let object = proc.get_object_with_rights::<Channel>(channel, rights);
    let handles = handle_buffer.take(proc, num_handles)?;
    if options & !USE_IOVEC != 0 {
        return Err(ZxError::INVALID_ARGS);
    }
    let object = object?;
    let data = if options & USE_IOVEC != 0 {
        read_channel_iovecs(proc, bytes, num_bytes)?
    } else {
        if num_bytes as usize > MAX_MESSAGE_BYTES {
            return Err(ZxError::OUT_OF_RANGE);
        }
        validate_user_range(proc, bytes.as_addr(), num_bytes as usize, MMUFlags::READ)?;
        bytes.read_array(num_bytes as usize)?
    };
    if is_call && data.len() < core::mem::size_of::<u32>() {
        return Err(ZxError::INVALID_ARGS);
    }
    let handles = handles.finish(proc, channel)?;
    Ok((object, MessagePacket { data, handles }))
}

struct ReplyBuffer {
    bytes: UserOutPtr<u8>,
    handles: usize,
    num_bytes: u32,
    num_handles: u32,
    is_etc: bool,
}

impl ReplyBuffer {
    fn write(
        mut self,
        proc: &Process,
        reply: MessagePacket,
        mut actual_bytes: UserOutPtr<u32>,
        mut actual_handles: UserOutPtr<u32>,
    ) -> ZxResult {
        let handle_size = if self.is_etc {
            core::mem::size_of::<HandleInfo>()
        } else {
            core::mem::size_of::<HandleValue>()
        };
        validate_user_range(
            proc,
            self.bytes.as_addr(),
            self.num_bytes as usize,
            MMUFlags::WRITE,
        )?;
        validate_user_range(
            proc,
            self.handles,
            self.num_handles as usize * handle_size,
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
        actual_bytes.write(reply.data.len() as u32)?;
        actual_handles.write(reply.handles.len() as u32)?;
        if (self.num_bytes as usize) < reply.data.len()
            || (self.num_handles as usize) < reply.handles.len()
        {
            return Err(ZxError::BUFFER_TOO_SMALL);
        }
        self.bytes.write_array(&reply.data)?;
        write_received_handles(proc, self.handles, reply.handles, self.is_etc)
    }
}

fn write_received_handles(
    proc: &Process,
    addr: usize,
    handles: Vec<Handle>,
    is_etc: bool,
) -> ZxResult {
    if is_etc {
        let mut infos: Vec<_> = handles.iter().map(Handle::get_handle_info).collect();
        for (info, value) in infos.iter_mut().zip(proc.add_handles(handles)) {
            info.handle = value;
        }
        UserOutPtr::<HandleInfo>::from(addr).write_array(&infos)?;
    } else {
        UserOutPtr::<HandleValue>::from(addr).write_array(&proc.add_handles(handles))?;
    }
    Ok(())
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
