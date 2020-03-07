use {super::*, zircon_object::signal::Port};

impl Syscall<'_> {
    pub fn sys_port_create(
        &self,
        options: u32,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        info!("port.create: options = {:#x}", options);
        if options != 0 {
            unimplemented!()
        }
        let port_handle = Handle::new(Port::new(), Rights::DEFAULT_PORT);
        let handle_value = self.thread.proc().add_handle(port_handle);
        out.write(handle_value)?;
        Ok(0)
    }
}
