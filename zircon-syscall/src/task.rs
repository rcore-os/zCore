use core::convert::TryFrom;
use {super::*, zircon_object::task::*};

impl Syscall<'_> {
    pub fn sys_process_create(
        &self,
        job: HandleValue,
        name: UserInPtr<u8>,
        name_size: usize,
        options: u32,
        mut proc_handle: UserOutPtr<HandleValue>,
        mut vmar_handle: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        let name = name.read_string(name_size)?;
        info!(
            "proc.create: job={:#x?}, name={:?}, options={:#x?}",
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
        Ok(())
    }

    pub fn sys_process_exit(&mut self, code: i64) -> ZxResult {
        info!("proc.exit: code={:?}", code);
        let proc = self.thread.proc();
        proc.exit(code);
        self.exit = true;
        Ok(())
    }

    pub fn sys_thread_create(
        &self,
        proc_handle: HandleValue,
        name: UserInPtr<u8>,
        name_size: usize,
        options: u32,
        mut thread_handle: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        let name = name.read_string(name_size)?;
        info!(
            "thread.create: proc={:#x?}, name={:?}, options={:#x?}",
            proc_handle, name, options,
        );
        assert_eq!(options, 0);
        let proc = self.thread.proc();
        let process = proc.get_object_with_rights::<Process>(proc_handle, Rights::MANAGE_THREAD)?;
        let thread = Thread::create(&process, &name, options)?;
        let handle = proc.add_handle(Handle::new(thread, Rights::DEFAULT_THREAD));
        thread_handle.write(handle)?;
        Ok(())
    }

    pub fn sys_process_start(
        &self,
        proc_handle: HandleValue,
        thread_handle: HandleValue,
        entry: usize,
        stack: usize,
        arg1_handle: HandleValue,
        arg2: usize,
    ) -> ZxResult {
        info!("process.start: proc_handle={:?}, thread_handle={:?}, entry={:?}, stack={:?}, arg1_handle={:?}, arg2={:?}",
            proc_handle, thread_handle, entry, stack, arg1_handle, arg2
        );
        let proc = self.thread.proc();
        let process = proc.get_object_with_rights::<Process>(proc_handle, Rights::WRITE)?;
        let thread = proc.get_object_with_rights::<Thread>(thread_handle, Rights::WRITE)?;
        if !Arc::ptr_eq(&thread.proc(), &process) {
            return Err(ZxError::ACCESS_DENIED);
        }
        let arg1 = if arg1_handle != INVALID_HANDLE {
            let arg1 = proc.remove_handle(arg1_handle)?;
            if !arg1.rights.contains(Rights::TRANSFER) {
                return Err(ZxError::ACCESS_DENIED);
            }
            process.add_handle(arg1)
        } else {
            arg1_handle
        };
        match thread.start(entry, stack, arg1 as usize, arg2) {
            Ok(()) => Ok(()),
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
    ) -> ZxResult {
        let kind = ThreadStateKind::try_from(kind).map_err(|_| ZxError::INVALID_ARGS)?;
        info!(
            "thread.write_state: handle={:#x?}, kind={:#x?}, buf=({:#x?}; {:#x?})",
            handle, kind, buffer, buffer_size,
        );
        let proc = self.thread.proc();
        let thread = proc.get_object_with_rights::<Thread>(handle, Rights::WRITE)?;
        let buf = buffer.read_array(buffer_size)?;
        thread.write_state(kind, &buf)?;
        Ok(())
    }

    pub fn sys_job_set_critical(
        &self,
        job_handle: HandleValue,
        options: u32,
        process_handle: HandleValue,
    ) -> ZxResult {
        info!(
            "job.set_critical: job={:#x?}, options={:#x}, process={:#x?}",
            job_handle, options, process_handle,
        );
        let retcode_nonzero = if options == 1 {
            true
        } else if options == 0 {
            false
        } else {
            unimplemented!()
        };
        let proc = self.thread.proc();
        let job = proc.get_object_with_rights::<Job>(job_handle, Rights::DESTROY)?;
        let process = proc.get_object_with_rights::<Process>(process_handle, Rights::WAIT)?;
        process.set_critical_job(job, retcode_nonzero)?;
        Ok(())
    }

    pub fn sys_thread_start(
        &self,
        handle_value: HandleValue,
        entry: usize,
        stack: usize,
        arg1: usize,
        arg2: usize,
    ) -> ZxResult {
        info!(
            "thread.start: handle={:#x?}, entry={:#x}, stack={:#x}, arg1={:#x} arg2={:#x}",
            handle_value, entry, stack, arg1, arg2
        );
        let thread = self
            .thread
            .proc()
            .get_object_with_rights::<Thread>(handle_value, Rights::MANAGE_THREAD)?;
        thread.start(entry, stack, arg1, arg2)?;
        Ok(())
    }

    pub fn sys_thread_exit(&mut self) -> ZxResult {
        self.thread.exit();
        self.exit = true;
        Ok(())
    }

    pub fn sys_task_suspend_token(
        &self,
        handle: HandleValue,
        mut token: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!("task.suspend_token: handle={:?}, token={:?}", handle, token);
        let proc = self.thread.proc();
        if let Ok(thread) = proc.get_object_with_rights::<Thread>(handle, Rights::WRITE) {
            if Arc::ptr_eq(&thread, &self.thread) {
                return Err(ZxError::NOT_SUPPORTED);
            }
            let token_handle =
                Handle::new(SuspendToken::create(&thread), Rights::DEFAULT_SUSPEND_TOKEN);
            token.write(proc.add_handle(token_handle))?;
            return Ok(());
        }
        if let Ok(process) = proc.get_object_with_rights::<Process>(handle, Rights::WRITE) {
            if Arc::ptr_eq(&process, &proc) {
                return Err(ZxError::NOT_SUPPORTED);
            }
            unimplemented!()
        }
        Ok(())
    }
}
