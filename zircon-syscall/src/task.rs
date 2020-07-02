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
        let job = proc
            .get_object_with_rights::<Job>(job, Rights::MANAGE_PROCESS)
            .or_else(|_| proc.get_object_with_rights::<Job>(job, Rights::WRITE))?;
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
            Some(arg1)
        } else {
            None
        };
        process.start(&thread, entry, stack, arg1, arg2, self.spawn_fn)?;
        Ok(())
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
        process.set_critical_at_job(&job, retcode_nonzero)?;
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
        let proc = self.thread.proc();
        let thread = proc.get_object_with_rights::<Thread>(handle_value, Rights::MANAGE_THREAD)?;
        if thread.proc().status() != Status::Running {
            return Err(ZxError::BAD_STATE);
        }
        thread.start(entry, stack, arg1, arg2, self.spawn_fn)?;
        Ok(())
    }

    pub fn sys_thread_exit(&mut self) -> ZxResult {
        info!("thread.exit:");
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
            let thread: Arc<dyn Task> = thread;
            let token_handle =
                Handle::new(SuspendToken::create(&thread), Rights::DEFAULT_SUSPEND_TOKEN);
            token.write(proc.add_handle(token_handle))?;
            return Ok(());
        }
        if let Ok(_process) = proc.get_object_with_rights::<Process>(handle, Rights::WRITE) {
            return Err(ZxError::WRONG_TYPE);
            // if Arc::ptr_eq(&process, &proc) {
            //     return Err(ZxError::NOT_SUPPORTED);
            // }
            // let proc_: Arc<dyn Task> = proc;
            // let token_handle =
            //     Handle::new(SuspendToken::create(&proc_), Rights::DEFAULT_SUSPEND_TOKEN);
            // token.write(proc.add_handle(token_handle))?;
            // return Ok(());
        }
        Ok(())
    }

    pub fn sys_task_kill(&self, handle: HandleValue) -> ZxResult {
        info!("task.kill: handle={:?}", handle);
        let proc = self.thread.proc();

        if let Ok(_job) = proc.get_object_with_rights::<Job>(handle, Rights::DESTROY) {
            // job.kill();
            return Err(ZxError::WRONG_TYPE);
        } else if let Ok(proc) = proc.get_object_with_rights::<Process>(handle, Rights::DESTROY) {
            proc.kill();
        } else if let Ok(thread) = proc.get_object_with_rights::<Thread>(handle, Rights::DESTROY) {
            info!(
                "killing thread: proc={:?} thread={:?}",
                thread.proc().name(),
                thread.name()
            );
            match thread.state() {
                ThreadState::Running | ThreadState::Suspended => {
                    if !Arc::ptr_eq(&thread, &self.thread) {
                        thread.kill();
                    }
                }
                _ => {
                    error!("{:?}", thread.state());
                }
            }
        } else {
            return Err(ZxError::WRONG_TYPE);
        }
        return Ok(());
    }

    pub fn sys_job_create(
        &self,
        parent: HandleValue,
        options: u32,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "job.create: parent={:#x}, options={:#x}, out={:#x?}",
            parent, options, out
        );
        if options != 0 {
            Err(ZxError::INVALID_ARGS)
        } else {
            let proc = self.thread.proc();
            let parent_job = proc
                .get_object_with_rights::<Job>(parent, Rights::MANAGE_JOB)
                .or_else(|_| proc.get_object_with_rights::<Job>(parent, Rights::WRITE))?;
            let child = parent_job.create_child(options)?;
            out.write(proc.add_handle(Handle::new(child, Rights::DEFAULT_JOB)))?;
            Ok(())
        }
    }

    pub fn sys_job_set_policy(
        &self,
        handle: HandleValue,
        options: u32,
        topic: u32,
        policy: usize,
        count: u32,
    ) -> ZxResult {
        info!(
            "job.set_policy: handle={:#x}, options={:#x}, topic={:#x}, policy={:#x?}, count={:#x}",
            handle, options, topic, policy, count,
        );
        let proc = self.thread.proc();
        let job = proc.get_object_with_rights::<Job>(handle, Rights::SET_POLICY)?;
        match topic {
            JOB_POL_BASE_V1 | JOB_POL_BASE_V2 => {
                let policy_option = match options {
                    JOB_POL_RELATIVE => SetPolicyOptions::Relative,
                    JOB_POL_ABSOLUTE => SetPolicyOptions::Absolute,
                    _ => return Err(ZxError::INVALID_ARGS),
                };
                let all_policy =
                    UserInPtr::<BasicPolicy>::from(policy).read_array(count as usize)?;
                job.set_policy_basic(policy_option, &all_policy)
            }
            //JOB_POL_BASE_V2 => unimplemented!(),
            JOB_POL_TIMER_SLACK => {
                if options != JOB_POL_RELATIVE {
                    return Err(ZxError::INVALID_ARGS);
                }
                if count != 1 {
                    return Err(ZxError::INVALID_ARGS);
                }
                let timer_policy = UserInPtr::<TimerSlackPolicy>::from(policy).read()?;
                job.set_policy_timer_slack(timer_policy)
            }
            _ => Err(ZxError::INVALID_ARGS),
        }
    }

    pub fn sys_process_read_memory(
        &self,
        handle_value: HandleValue,
        vaddr: usize,
        mut buffer: UserOutPtr<u8>,
        buffer_size: usize,
        mut actual: UserOutPtr<u32>,
    ) -> ZxResult {
        if buffer.is_null() || buffer_size == 0 || buffer_size > MAX_BLOCK {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let process =
            proc.get_object_with_rights::<Process>(handle_value, Rights::READ | Rights::WRITE)?;
        let mut data = vec![0u8; buffer_size];
        let len = process.vmar().read_memory(vaddr, &mut data)?;
        buffer.write_array(&data[..len])?;
        actual.write_if_not_null(len as u32)?;
        Ok(())
    }

    pub fn sys_process_write_memory(
        &self,
        handle_value: HandleValue,
        vaddr: usize,
        buffer: UserInPtr<u8>,
        buffer_size: usize,
        mut actual: UserOutPtr<u32>,
    ) -> ZxResult {
        if buffer.is_null() || buffer_size == 0 || buffer_size > MAX_BLOCK {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let process =
            proc.get_object_with_rights::<Process>(handle_value, Rights::READ | Rights::WRITE)?;
        let data = buffer.read_array(buffer_size)?;
        let len = process.vmar().write_memory(vaddr, &data)?;
        actual.write_if_not_null(len as u32)?;
        Ok(())
    }
}

const JOB_POL_BASE_V1: u32 = 0;
const JOB_POL_BASE_V2: u32 = 0x0100_0000;
const JOB_POL_TIMER_SLACK: u32 = 1;

const JOB_POL_RELATIVE: u32 = 0;
const JOB_POL_ABSOLUTE: u32 = 1;

const MAX_BLOCK: usize = 64 * 1024 * 1024; //64M
