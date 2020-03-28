use {super::*, zircon_object::signal::*};

impl Syscall<'_> {
    pub fn sys_port_create(
        &self,
        options: u32,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        info!("port.create: options={:#x}", options);
        if options != 0 {
            unimplemented!()
        }
        let port_handle = Handle::new(Port::new(), Rights::DEFAULT_PORT);
        let handle_value = self.thread.proc().add_handle(port_handle);
        out.write(handle_value)?;
        Ok(0)
    }

    pub async fn sys_port_wait(
        &self,
        handle_value: HandleValue,
        deadline: u64,
        mut packet_res: UserOutPtr<PortPacket>,
    ) -> ZxResult<usize> {
        info!(
            "port.wait: handle={}, deadline={:#x}",
            handle_value, deadline
        );
        assert_eq!(core::mem::size_of::<PortPacket>(), 48);
        let proc = self.thread.proc();
        let port = proc.get_object_with_rights::<Port>(handle_value, Rights::READ)?;
        let packet = port.wait().await;
        debug!("port.wait: packet={:#x?}", packet);
        packet_res.write(packet)?;
        Ok(0)
    }

    pub fn sys_port_queue(
        &self,
        handle_value: HandleValue,
        packcet_in: UserInPtr<PortPacket>,
    ) -> ZxResult<usize> {
        // TODO when to return ZX_ERR_SHOULD_WAIT
        let proc = self.thread.proc();
        let port = proc.get_object_with_rights::<Port>(handle_value, Rights::WRITE)?;
        let packet = packcet_in.read()?;
        info!(
            "port.queue: handle={:#x}, packet={:?}",
            handle_value, packet
        );
        port.push(packet);
        Ok(0)
    }
}
