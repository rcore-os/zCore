use {super::*, zircon_object::task::*};

impl Syscall {
    pub fn sys_process_create(
        &self,
        job: HandleValue,
        name: UserInPtr<u8>,
        name_size: usize,
        options: u32,
        proc_handle: UserOutPtr<HandleValue>,
        vmar_handle: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        let name = name.read_string(name_size)?;
        info!(
            "proc.create: job={:?}, name={:?}, options={:?}",
            job, name, options,
        );
        let proc = &self.thread.proc;
        let job = proc.get_object_with_rights::<Job>(job, Rights::MANAGE_PROCESS)?;
        let new_proc = Process::create(&job, &name, options)?;
        let new_vmar = new_proc.vmar();
        let proc_handle_value = proc.add_handle(Handle::new(new_proc, Rights::DEFAULT_PROCESS));
        let vmar_handle_value = proc.add_handle(Handle::new(new_vmar, Rights::DEFAULT_VMAR));
        proc_handle.write(proc_handle_value)?;
        vmar_handle.write(vmar_handle_value)?;
        Ok(0)
    }

    pub fn sys_process_exit(&self, code: i64) -> ZxResult<usize> {
        panic!("proc.exit: code={:?}", code);
    }
}
