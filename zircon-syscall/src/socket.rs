use {super::*, bitflags::bitflags, zircon_object::ipc::*};

impl Syscall<'_> {
    pub fn sys_socket_create(
        &self,
        options: u32,
        mut out0: UserOutPtr<HandleValue>,
        mut out1: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!("socket.create: options={:#x?}", options);
        if options != 0 {
            error!("socket.create: only implemented options=0");
            return Err(ZxError::NOT_SUPPORTED);
        }
        let (end0, end1) = Socket::create();
        let proc = self.thread.proc();
        let handle0 = proc.add_handle(Handle::new(end0, Rights::DEFAULT_SOCKET));
        let handle1 = proc.add_handle(Handle::new(end1, Rights::DEFAULT_SOCKET));
        out0.write(handle0)?;
        out1.write(handle1)?;
        Ok(())
    }

    pub fn sys_socket_write(
        &self,
        socket: HandleValue,
        options: u32,
        buffer: UserInPtr<u8>,
        size: usize,
        mut actual_size: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "socket.write: socket={:#x?}, options={:#x?}, buffer={:#x?}, size={:#x?}",
            socket, options, buffer, size,
        );
        if options != 0 {
            unimplemented!();
        }
        let proc = self.thread.proc();
        let socket = proc.get_object_with_rights::<Socket>(socket, Rights::WRITE)?;
        let buffer = buffer.read_array(size)?;
        let size = socket.write(&buffer)?;
        actual_size.write_if_not_null(size)?;
        Ok(())
    }

    pub fn sys_socket_read(
        &self,
        socket: HandleValue,
        options: u32,
        mut buffer: UserOutPtr<u8>,
        size: usize,
        mut actual_size: UserOutPtr<usize>,
    ) -> ZxResult {
        let options = SocketOptions::from_bits_truncate(options);
        info!(
            "socket.read: socket={:#x?}, options={:#x?}, buffer={:#x?}, size={:#x?}",
            socket, options, buffer, size,
        );
        let proc = self.thread.proc();
        let socket = proc.get_object_with_rights::<Socket>(socket, Rights::READ)?;
        let peek = options.contains(SocketOptions::PEEK);
        let result = socket.read(size, peek)?;
        actual_size.write_if_not_null(result.len())?;
        buffer.write_array(&result)?;
        Ok(())
    }

    pub fn sys_socket_shutdown(&self, socket: HandleValue, options: u32) -> ZxResult {
        let options = SocketOptions::from_bits_truncate(options);
        info!(
            "socket.shutdown: socket={:#x?}, options={:#x?}",
            socket, options
        );
        let proc = self.thread.proc();
        let socket = proc.get_object_with_rights::<Socket>(socket, Rights::WRITE)?;
        let read = options.contains(SocketOptions::SHUTDOWN_READ);
        let write = options.contains(SocketOptions::SHUTDOWN_WRITE);
        socket.shutdown(read, write)?;
        Ok(())
    }
}

bitflags! {
    #[derive(Default)]
    struct SocketOptions: u32 {
        #[allow(clippy::identity_op)]
        const SHUTDOWN_WRITE = 1 << 0;
        const SHUTDOWN_READ = 1 << 1;
        #[allow(clippy::identity_op)]
        const DATAGRAM = 1 << 0;
        const PEEK = 1 << 3;
    }
}
