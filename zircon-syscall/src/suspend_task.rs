use {super::*, zircon_object::task::*};

impl Syscall {
    pub fn sys_task_suspend_token(
        &self,
        handle: HandleValue,
        mut token: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        info!("task.suspend_token: handle={:?}, token={:?}", handle, token);
        let proc = self.thread.proc();
        if let Ok(thread) = proc.get_object_with_rights::<Thread>(handle, Rights::WRITE) {
            if Arc::ptr_eq(&thread, &self.thread) {
                return Err(ZxError::NOT_SUPPORTED);
            }
            let token_handle =
                Handle::new(SuspendToken::create(&thread), Rights::DEFAULT_SUSPEND_TOKEN);
            token.write(proc.add_handle(token_handle))?;
            return Ok(0);
        }
        if let Ok(process) = proc.get_object_with_rights::<Process>(handle, Rights::WRITE) {
            if Arc::ptr_eq(&process, &proc) {
                return Err(ZxError::NOT_SUPPORTED);
            }
            unimplemented!()
        }
        Ok(0)
    }
}
