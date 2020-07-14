use {super::*, numeric_enum_macro::numeric_enum, zircon_object::task::*};

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
        let (task, rights) = proc.get_dyn_object_and_rights(task)?;
        if !rights.contains(
            Rights::INSPECT | Rights::DUPLICATE | Rights::TRANSFER | Rights::MANAGE_THREAD,
        ) {
            return Err(ZxError::ACCESS_DENIED);
        }
        let option = ExceptionChannelOption::try_from(option).map_err(|_| ZxError::INVALID_ARGS)?;
        let exceptionate = if let Ok(job) = task.clone().downcast_arc::<Job>() {
            if !rights.contains(Rights::ENUMERATE) {
                return Err(ZxError::ACCESS_DENIED);
            }
            match option {
                ExceptionChannelOption::None => job.get_exceptionate(),
                ExceptionChannelOption::Debugger => job.get_debug_exceptionate(),
            }
        } else if let Ok(process) = task.clone().downcast_arc::<Process>() {
            if !rights.contains(Rights::ENUMERATE) {
                return Err(ZxError::ACCESS_DENIED);
            }
            match option {
                ExceptionChannelOption::None => process.get_exceptionate(),
                ExceptionChannelOption::Debugger => process.get_debug_exceptionate(),
            }
        } else if let Ok(thread) = task.clone().downcast_arc::<Thread>() {
            match option {
                ExceptionChannelOption::None => thread.get_exceptionate(),
                ExceptionChannelOption::Debugger => return Err(ZxError::INVALID_ARGS),
            }
        } else {
            return Err(ZxError::WRONG_TYPE);
        };
        let user_end = proc.add_handle(Handle::new(
            exceptionate.create_channel()?,
            Rights::TRANSFER | Rights::WAIT | Rights::READ,
        ));
        out.write(user_end)?;
        Ok(())
    }

    pub fn sys_exception_get_thread(
        &self,
        exception: HandleValue,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        let proc = self.thread.proc();
        let exception =
            proc.get_object_with_rights::<ExceptionObject>(exception, Rights::default())?;
        let (object, right) = exception.get_exception().get_thread_and_rights();
        let handle = proc.add_handle(Handle::new(object, right));
        out.write(handle)?;
        Ok(())
    }

    pub fn sys_exception_get_process(
        &self,
        exception: HandleValue,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        let proc = self.thread.proc();
        let exception =
            proc.get_object_with_rights::<ExceptionObject>(exception, Rights::default())?;
        if exception.get_exception().get_current_channel_type() == ExceptionChannelType::Thread {
            return Err(ZxError::ACCESS_DENIED)
        }
        let (object, right) = exception.get_exception().get_process_and_rights();
        let handle = proc.add_handle(Handle::new(object, right));
        out.write(handle)?;
        Ok(())
    }
}

numeric_enum! {
    #[repr(u32)]
    pub enum ExceptionChannelOption {
        None = 0,
        Debugger = 1
    }
}
