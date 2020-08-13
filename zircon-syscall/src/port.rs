use {
    super::*,
    zircon_object::{signal::*, task::*},
};

impl Syscall<'_> {
    /// Create an IO port.  
    pub fn sys_port_create(&self, options: u32, mut out: UserOutPtr<HandleValue>) -> ZxResult {
        info!("port.create: options={:#x}", options);
        let port_handle = Handle::new(Port::new(options)?, Rights::DEFAULT_PORT);
        let handle_value = self.thread.proc().add_handle(port_handle);
        out.write(handle_value)?;
        Ok(())
    }

    /// Wait for a packet arrival in a port.  
    pub async fn sys_port_wait(
        &self,
        handle_value: HandleValue,
        deadline: Deadline,
        mut packet_res: UserOutPtr<PortPacket>,
    ) -> ZxResult {
        info!(
            "port.wait: handle={}, deadline={:?}",
            handle_value, deadline
        );
        assert_eq!(core::mem::size_of::<PortPacket>(), 48);
        let proc = self.thread.proc();
        let port = proc.get_object_with_rights::<Port>(handle_value, Rights::READ)?;
        let future = port.wait();
        pin_mut!(future);
        let packet = self
            .thread
            .blocking_run(future, ThreadState::BlockedPort, deadline.into(), None)
            .await?;
        packet_res.write(packet)?;
        Ok(())
    }

    /// Queue a packet to a port.  
    pub fn sys_port_queue(
        &self,
        handle_value: HandleValue,
        packcet_in: UserInPtr<PortPacket>,
    ) -> ZxResult {
        let proc = self.thread.proc();
        let port = proc.get_object_with_rights::<Port>(handle_value, Rights::WRITE)?;
        let packet = packcet_in.read()?;
        info!(
            "port.queue: handle={:#x}, packet={:?}",
            handle_value, packet
        );
        port.push_user(packet)?;
        Ok(())
    }
}
