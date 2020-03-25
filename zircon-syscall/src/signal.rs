use {
    super::*,
    core::time::Duration,
    zircon_object::{signal::*, task::PolicyCondition},
};

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

    pub fn sys_timer_create(
        &self,
        options: u32,
        clock_id: u32,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        info!(
            "timer.create: options = {:#x}, clock_id={:#x}",
            options, clock_id
        );
        if clock_id != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        proc.check_policy(PolicyCondition::NewTimer)?;
        let handle = Handle::new(Timer::create(options)?, Rights::DEFAULT_TIMER);
        out.write(proc.add_handle(handle))?;
        Ok(0)
    }

    pub fn sys_event_create(
        &self,
        options: u32,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        info!("event.create: options = {:#x}", options);
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        proc.check_policy(PolicyCondition::NewEvent)?;
        let handle = Handle::new(Event::new(), Rights::DEFAULT_EVENT);
        out.write(proc.add_handle(handle))?;
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
        let port = self
            .thread
            .proc()
            .get_object_with_rights::<Port>(handle_value, Rights::READ)?;
        let packet = port.wait_async().await;
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
        let port = self
            .thread
            .proc()
            .get_object_with_rights::<Port>(handle_value, Rights::WRITE)?;
        let packet = packcet_in.read()?;
        info!(
            "port.queue: handle={:#x}, packet={:?}",
            handle_value, packet
        );
        port.push(packet);
        Ok(0)
    }

    pub async fn sys_nanosleep(&self, deadline: i64) -> ZxResult<usize> {
        if deadline <= 0 {
            // just yield current thread
            let yield_future = YieldFutureImpl::default();
            yield_future.await;
        } else {
            let state = SleepState::new();
            state.set_deadline(Duration::from_nanos(deadline as u64));
            SleepFutureImpl { state }.await;
        }
        Ok(0)
    }
}

