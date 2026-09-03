use {super::*, zircon_object::object::Counter};

impl Syscall<'_> {
    pub fn sys_counter_create(&self, options: u32, mut out: UserOutPtr<HandleValue>) -> ZxResult {
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let handle = self
            .thread
            .proc()
            .add_handle(Handle::new(Counter::new(), Rights::DEFAULT_COUNTER));
        out.write(handle)?;
        Ok(())
    }

    pub fn sys_counter_add(&self, handle: HandleValue, amount: i64) -> ZxResult {
        let (counter, rights) = self
            .thread
            .proc()
            .get_object_and_rights::<Counter>(handle)?;
        if !rights.contains(Rights::READ | Rights::WRITE) {
            return Err(ZxError::ACCESS_DENIED);
        }
        counter.add(amount)
    }

    pub fn sys_counter_read(&self, handle: HandleValue, mut out: UserOutPtr<i64>) -> ZxResult {
        let proc = self.thread.proc();
        let (counter, rights) = proc.get_object_and_rights::<Counter>(handle)?;
        if !rights.contains(Rights::READ) {
            return Err(ZxError::ACCESS_DENIED);
        }
        crate::channel::validate_user_range(
            proc,
            out.as_addr(),
            core::mem::size_of::<i64>(),
            kernel_hal::MMUFlags::WRITE,
        )?;
        let value = counter.read();
        out.write(value)?;
        Ok(())
    }

    pub fn sys_counter_write(&self, handle: HandleValue, value: i64) -> ZxResult {
        let (counter, rights) = self
            .thread
            .proc()
            .get_object_and_rights::<Counter>(handle)?;
        if !rights.contains(Rights::WRITE) {
            return Err(ZxError::ACCESS_DENIED);
        }
        counter.write(value);
        Ok(())
    }
}
