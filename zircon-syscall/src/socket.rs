use {super::*, zircon_object::ipc::Socket};

impl Syscall<'_> {
    pub fn sys_socket_create(
        &self,
        options: u32,
        mut out0: UserOutPtr<HandleValue>,
        mut out1: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        error!("socket.create: options={:#x?}", options);
        if options != 0 {
            unimplemented!();
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
        error!(
            "socket.write: socket={:#x?} options={:#x?} buffer={:#x?} size={:#x?}",
            socket, options, buffer, size,
        );
        let socket = self.thread.proc().get_object_with_rights::<Socket>(socket, Rights::WRITE)?;
        let buffer = buffer.read_array(size)?;
        let size = socket.write(buffer)?;
        actual_size.write(size)?;
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
        error!(
            "socket.read: socket={:#x?} options={:#x?} buffer={:#x?} size={:#x?}",
            socket, options, buffer, size,
        );
        let socket = self.thread.proc().get_object_with_rights::<Socket>(socket, Rights::READ)?;
        let result = socket.read(size)?;
        actual_size.write(result.len())?;
        buffer.write_array(&result)?;
        Ok(())
    }

    pub fn sys_socket_shutdown(&self, socket: HandleValue, options: u32) -> ZxResult {
        error!("socket.shutdown: socket={:#x?} options={:#x?}", socket, options);
        let socket = self.thread.proc().get_object_with_rights::<Socket>(socket, Rights::WRITE)?;
        socket.shutdown(options)?;
        Ok(())
    }
}
