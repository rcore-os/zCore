use {super::*, kernel_hal::MMUFlags, zircon_object::ipc::Socket, zircon_object::ipc::SocketFlags};

impl Syscall<'_> {
    /// Create a socket.
    ///
    /// Socket is a connected pair of bidirectional stream transports, that can move only data, and that have a maximum capacity.
    pub fn sys_socket_create(
        &self,
        options: u32,
        mut out0: UserOutPtr<HandleValue>,
        mut out1: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!("socket.create: options={:#x?}", options);
        let (end0, end1) = Socket::create(options)?;
        let proc = self.thread.proc();
        let handle0 = proc.add_handle(Handle::new(end0, Rights::DEFAULT_SOCKET));
        let handle1 = proc.add_handle(Handle::new(end1, Rights::DEFAULT_SOCKET));
        out0.write(handle0)?;
        out1.write(handle1)?;
        Ok(())
    }

    /// Write data to a socket.
    ///
    /// Attempts to write `count: usize` bytes to the socket specified by `handle_value`.
    pub fn sys_socket_write(
        &self,
        handle_value: HandleValue,
        options: u32,
        user_bytes: UserInPtr<u8>,
        count: usize,
        mut actual_count_ptr: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "socket.write: socket={:#x?}, options={:#x?}, buffer={:#x?}, size={:#x?}",
            handle_value, options, user_bytes, count,
        );
        if (count == 0 || !user_bytes.is_null()) && options == 0 {
            let proc = self.thread.proc();
            let socket = proc.get_object_with_rights::<Socket>(handle_value, Rights::WRITE)?;
            let write_size = socket.write_size(count)?;
            crate::channel::validate_user_range(
                proc,
                user_bytes.as_addr(),
                write_size,
                MMUFlags::READ,
            )?;
            if !actual_count_ptr.is_null() {
                crate::channel::validate_user_range(
                    proc,
                    actual_count_ptr.as_addr(),
                    core::mem::size_of::<usize>(),
                    MMUFlags::WRITE,
                )?;
            }
            let actual_count = socket.write(user_bytes.as_slice(write_size)?)?;
            actual_count_ptr.write_if_not_null(actual_count)?;
            Ok(())
        } else {
            Err(ZxError::INVALID_ARGS)
        }
    }

    /// Read data from a socket.
    pub fn sys_socket_read(
        &self,
        handle_value: HandleValue,
        options: u32,
        mut user_bytes: UserOutPtr<u8>,
        count: usize,
        mut actual_count_ptr: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "socket.read: socket={:#x?}, options={:#x?}, buffer={:#x?}, size={:#x?}",
            handle_value, options, user_bytes, count,
        );
        if count > 0 && user_bytes.is_null() {
            return Err(ZxError::INVALID_ARGS);
        }
        let options = SocketFlags::from_bits(options).ok_or(ZxError::INVALID_ARGS)?;
        if !(options - SocketFlags::SOCKET_PEEK).is_empty() {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let socket = proc.get_object_with_rights::<Socket>(handle_value, Rights::READ)?;
        crate::channel::validate_user_range(proc, user_bytes.as_addr(), count, MMUFlags::WRITE)?;
        if !actual_count_ptr.is_null() {
            crate::channel::validate_user_range(
                proc,
                actual_count_ptr.as_addr(),
                core::mem::size_of::<usize>(),
                MMUFlags::WRITE,
            )?;
        }
        let mut data = vec![0; count];
        let peek = options.contains(SocketFlags::SOCKET_PEEK);
        let actual_count = socket.read(peek, &mut data)?;
        user_bytes.write_array(&data[..actual_count])?;
        actual_count_ptr.write_if_not_null(actual_count)?;
        Ok(())
    }

    /// Change the write disposition of this endpoint and/or its peer.
    pub fn sys_socket_set_disposition(
        &self,
        handle: HandleValue,
        disposition: u32,
        peer_disposition: u32,
    ) -> ZxResult {
        const NONE: u32 = 0;
        const WRITE_DISABLED: u32 = 1;
        const WRITE_ENABLED: u32 = 2;
        if !matches!(disposition, NONE | WRITE_DISABLED | WRITE_ENABLED)
            || !matches!(peer_disposition, NONE | WRITE_DISABLED | WRITE_ENABLED)
        {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let socket = proc.get_object_with_rights::<Socket>(handle, Rights::MANAGE_SOCKET)?;
        let decode = |value| match value {
            WRITE_DISABLED => Some(true),
            WRITE_ENABLED => Some(false),
            _ => None,
        };
        socket.set_disposition(decode(disposition), decode(peer_disposition))
    }

    /// Prevent future reading or writing on a socket.
    pub fn sys_socket_shutdown(&self, socket: HandleValue, options: u32) -> ZxResult {
        let options = SocketFlags::from_bits_truncate(options);
        info!(
            "socket.shutdown: socket={:#x?}, options={:#x?}",
            socket, options
        );
        let proc = self.thread.proc();
        let socket = proc.get_object_with_rights::<Socket>(socket, Rights::WRITE)?;
        let read = options.contains(SocketFlags::SHUTDOWN_READ);
        let write = options.contains(SocketFlags::SHUTDOWN_WRITE);
        socket.shutdown(read, write)?;
        Ok(())
    }
}
