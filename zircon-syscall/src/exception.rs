use {
    super::*,
    zircon_object::{ipc::Channel, task::*},
};

impl Syscall<'_> {
    pub fn sys_create_exception_channel(
        &self,
        task: HandleValue,
        option: u32,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "create_exception_channel: task={:#x}, options={:#x}, out={:#x?}",
            task, option, out
        );
        let proc = self.thread.proc();
        let task = proc.get_dyn_object_with_rights(
            task,
            Rights::INSPECT | Rights::DUPLICATE | Rights::TRANSFER | Rights::MANAGE_THREAD,
        )?;
        let exceptionate = task
            .clone()
            .downcast_arc::<Job>()
            .map(|x| x.get_exceptionate())
            .or_else(|_| {
                task.clone()
                    .downcast_arc::<Process>()
                    .map(|x| x.get_exceptionate())
            })
            .or_else(|_| {
                task.clone()
                    .downcast_arc::<Thread>()
                    .map(|x| x.get_exceptionate())
            })
            .map_err(|_| ZxError::WRONG_TYPE)?;
        let (end0, end1) = Channel::create();
        exceptionate.set_channel(end0);
        let user_end = proc.add_handle(Handle::new(end1, Rights::DEFAULT_CHANNEL));
        out.write(user_end)?;
        Ok(())
    }
}
