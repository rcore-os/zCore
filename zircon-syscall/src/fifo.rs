use {
    super::*,
    zircon_object::ipc::Fifo,
};

impl Syscall<'_> {
    pub fn sys_fifo_create(
        &self,
        count: usize,
        item_size: usize,
        _options: u32,
        mut out0: UserOutPtr<HandleValue>,
        mut out1: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        let (end0, end1) = Fifo::create(count, item_size);
        let proc = self.thread.proc();
        let handle0 = proc.add_handle(Handle::new(end0, Rights::DEFAULT_FIFO));
        let handle1 = proc.add_handle(Handle::new(end1, Rights::DEFAULT_FIFO));
        out0.write(handle0)?;
        out1.write(handle1)?;
        Ok(())
    }
}
