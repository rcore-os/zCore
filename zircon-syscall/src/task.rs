use {super::*, zircon_object::task::*};

impl Syscall {
    pub fn sys_process_create(
        &self,
        job: HandleValue,
        name: UserInPtr<u8>,
        name_size: usize,
        options: u32,
        mut proc_handle: UserOutPtr<HandleValue>,
        mut vmar_handle: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        let name = name.read_string(name_size)?;
        info!(
            "proc.create: job={:?}, name={:?}, options={:?}",
            job, name, options,
        );
        let proc = self.thread.proc();
        let job = proc.get_object_with_rights::<Job>(job, Rights::MANAGE_PROCESS)?;
        let new_proc = Process::create(&job, &name, options)?;
        let new_vmar = new_proc.vmar();
        let proc_handle_value = proc.add_handle(Handle::new(new_proc, Rights::DEFAULT_PROCESS));
        let vmar_handle_value = proc.add_handle(Handle::new(
            new_vmar,
            Rights::DEFAULT_VMAR | Rights::READ | Rights::WRITE | Rights::EXECUTE,
        ));
        proc_handle.write(proc_handle_value)?;
        vmar_handle.write(vmar_handle_value)?;
        Ok(0)
    }

    pub fn sys_process_exit(&mut self, code: i64) -> ZxResult<usize> {
        info!("proc.exit: code={:?}", code);
        let proc = self.thread.proc();
        proc.exit(code);
        self.exit = true;
        Ok(0)
    }

    pub fn sys_thread_create(
        &self,
        proc_handle: HandleValue,
        name: UserInPtr<u8>,
        name_size: usize,
        options: u32,
        mut thread_handle: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        let name = name.read_string(name_size)?;
        info!(
            "thread.create: proc={:?}, name={:?}, options={:?}",
            proc_handle, name, options,
        );
        assert_eq!(options, 0);
        let proc = self.thread.proc();
        let process = proc.get_object_with_rights::<Process>(proc_handle, Rights::MANAGE_THREAD)?;
        let thread = Thread::create(&process, &name, options)?;
        let handle = proc.add_handle(Handle::new(thread, Rights::DEFAULT_THREAD));
        thread_handle.write(handle)?;
        Ok(0)
    }

    pub fn sys_process_start(
        &self,
        proc_handle: HandleValue,
        thread_handle: HandleValue,
        entry: usize,
        stack: usize,
        arg1_handle: HandleValue,
        arg2: usize,
    ) -> ZxResult<usize> {
        info!("process.start: proc_handle = {:?}, thread_handle = {:?}, entry = {:?}, stack = {:?}, arg1_handle = {:?}, arg2 = {:?}",
            proc_handle, thread_handle, entry, stack, arg1_handle, arg2
        );
        let proc = self.thread.proc();
        let arg1_handle = proc.remove_handle(arg1_handle)?;
        let process = proc.get_object_with_rights::<Process>(proc_handle, Rights::WRITE)?;
        let thread = proc.get_object_with_rights::<Thread>(thread_handle, Rights::WRITE)?;
        if !Arc::ptr_eq(&thread.proc(), &process) {
            return Err(ZxError::ACCESS_DENIED);
        }
        if !arg1_handle.rights.contains(Rights::TRANSFER) {
            return Err(ZxError::ACCESS_DENIED);
        }
        let arg1 = process.add_handle(arg1_handle);
        match thread.start(entry, stack, arg1 as usize, arg2) {
            Ok(()) => Ok(0),
            Err(e) => {
                process.remove_handle(arg1)?;
                Err(e)
            }
        }
    }

    pub fn sys_thread_write_state(
        &self,
        handle: HandleValue,
        kind: u32,
        buffer: UserInPtr<u8>,
        buffer_size: usize,
    ) -> ZxResult<usize> {
        let proc = self.thread.proc();
        let thread = proc.get_object_with_rights::<Thread>(handle, Rights::WRITE)?;
        let buf = buffer.read_array(buffer_size)?;
        assert_eq!(kind, 0u32);
        thread.write_state(ThreadStateKind::General, &buf)?;
        Ok(0)
    }
}
