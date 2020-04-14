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
        let task = proc.get_object_with_rights::<Job>(
            task,
            Rights::INSPECT | Rights::DUPLICATE | Rights::TRANSFER | Rights::MANAGE_THREAD,
        )?;
        let exceptionate = task.get_exceptionate();
        let (end0, end1) = Channel::create();
        exceptionate.set_channel(end0);
        let user_end = proc.add_handle(Handle::new(end1, Rights::DEFAULT_CHANNEL));
        out.write(user_end)?;
        Ok(())
    }
}
