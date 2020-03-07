use {
    super::*,
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
}
