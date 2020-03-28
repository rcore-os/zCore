use {
    super::*,
    zircon_object::{signal::*, task::PolicyCondition},
};

impl Syscall<'_> {
    pub fn sys_timer_create(
        &self,
        options: u32,
        clock_id: u32,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        info!(
            "timer.create: options={:#x}, clock_id={:#x}",
            options, clock_id
        );
        if clock_id != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        proc.check_policy(PolicyCondition::NewTimer)?;
        let slack = match options {
            0 => Slack::Center,
            1 => Slack::Early,
            2 => Slack::Late,
            _ => return Err(ZxError::INVALID_ARGS),
        };
        let handle = Handle::new(Timer::with_slack(slack), Rights::DEFAULT_TIMER);
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
