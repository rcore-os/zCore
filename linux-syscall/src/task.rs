//! Syscalls for process

use super::*;
use bitflags::bitflags;
use linux_object::fs::INodeExt;
use linux_object::loader::LinuxElfLoader;
use linux_object::thread::ThreadExt;

impl Syscall<'_> {
    /// Fork the current process. Return the child's PID.
    pub fn sys_fork(&self) -> SysResult {
        info!("fork:");
        let new_proc = Process::fork_from(self.zircon_process(), false)?;
        let new_thread = Thread::create_linux(&new_proc)?;
        new_thread.start_with_context(UserContext::new_fork(self.context), self.spawn_fn)?;

        info!("fork: {} -> {}", self.zircon_process().id(), new_proc.id());
        Ok(new_proc.id() as usize)
    }

    pub async fn sys_vfork(&self) -> SysResult {
        info!("vfork:");
        let new_proc = Process::fork_from(self.zircon_process(), true)?;
        let new_thread = Thread::create_linux(&new_proc)?;
        new_thread.start_with_context(UserContext::new_fork(self.context), self.spawn_fn)?;

        let new_proc: Arc<dyn KernelObject> = new_proc;
        info!("vfork: {} -> {}", self.zircon_process().id(), new_proc.id());
        new_proc.wait_signal(Signal::SIGNALED).await; // wait for execve
        Ok(new_proc.id() as usize)
    }

    /// Create a new thread in the current process.
    /// The new thread's stack pointer will be set to `newsp`,
    /// and thread pointer will be set to `newtls`.
    /// The child tid will be stored at both `parent_tid` and `child_tid`.
    /// This is partially implemented for musl only.
    pub fn sys_clone(
        &self,
        flags: usize,
        newsp: usize,
        mut parent_tid: UserOutPtr<i32>,
        mut child_tid: UserOutPtr<i32>,
        newtls: usize,
    ) -> SysResult {
        let _flags = CloneFlags::from_bits_truncate(flags);
        info!(
            "clone: flags={:#x}, newsp={:#x}, parent_tid={:?}, child_tid={:?}, newtls={:#x}",
            flags, newsp, parent_tid, child_tid, newtls
        );
        if flags == 0x4111 || flags == 0x11 {
            warn!("sys_clone is calling sys_fork instead, ignoring other args");
            unimplemented!()
            //            return self.sys_fork();
        }
        if flags != 0x7d_0f00 && flags != 0x5d_0f00 {
            // 0x5d0f00: gcc of alpine linux
            // 0x7d0f00: pthread_create of alpine linux
            // warn!("sys_clone only support musl pthread_create");
            panic!("unsupported sys_clone flags: {:#x}", flags);
        }
        let new_thread = Thread::create_linux(self.zircon_process())?;
        let context = UserContext::new_clone(self.context, newsp, newtls);
        new_thread.start_with_context(context, self.spawn_fn)?;

        let tid = new_thread.id();
        info!("clone: {} -> {}", self.thread.id(), tid);
        parent_tid.write(tid as i32)?;
        child_tid.write(tid as i32)?;
        new_thread.set_tid_address(child_tid);
        Ok(tid as usize)
    }

    /// Wait for a child process exited.
    ///
    /// Return the PID. Store exit code to `wstatus` if it's not null.
    pub async fn sys_wait4(
        &self,
        pid: i32,
        mut wstatus: UserOutPtr<i32>,
        options: u32,
    ) -> SysResult {
        #[derive(Debug)]
        enum WaitTarget {
            AnyChild,
            AnyChildInGroup,
            Pid(KoID),
        }
        bitflags! {
            struct WaitFlags: u32 {
                const NOHANG    = 1;
                const STOPPED   = 2;
                const EXITED    = 4;
                const CONTINUED = 8;
                const NOWAIT    = 0x100_0000;
            }
        }
        let target = match pid {
            -1 => WaitTarget::AnyChild,
            0 => WaitTarget::AnyChildInGroup,
            p if p > 0 => WaitTarget::Pid(p as KoID),
            _ => unimplemented!(),
        };
        let flags = WaitFlags::from_bits_truncate(options);
        let nohang = flags.contains(WaitFlags::NOHANG);
        info!(
            "wait4: target={:?}, wstatus={:?}, options={:?}",
            target, wstatus, flags,
        );
        let (pid, code) = match target {
            WaitTarget::AnyChild | WaitTarget::AnyChildInGroup => {
                wait_child_any(self.zircon_process(), nohang).await?
            }
            WaitTarget::Pid(pid) => (pid, wait_child(self.zircon_process(), pid, nohang).await?),
        };
        wstatus.write_if_not_null(code)?;
        Ok(pid as usize)
    }

    /// Replaces the current ** process ** with a new process image
    ///
    /// `argv` is an array of argument strings passed to the new program.
    /// `envp` is an array of strings, conventionally of the form `key=value`,
    /// which are passed as environment to the new program.
    ///
    /// NOTICE: `argv` & `envp` can not be NULL (different from Linux)
    ///
    /// NOTICE: for multi-thread programs
    /// A call to any exec function from a process with more than one thread
    /// shall result in all threads being terminated and the new executable image
    /// being loaded and executed.
    pub fn sys_execve(
        &mut self,
        path: UserInPtr<u8>,
        argv: UserInPtr<UserInPtr<u8>>,
        envp: UserInPtr<UserInPtr<u8>>,
    ) -> SysResult {
        info!(
            "execve: path: {:?}, argv: {:?}, envp: {:?}",
            path, argv, envp
        );
        let path = path.read_cstring()?;
        let args = argv.read_cstring_array()?;
        let envs = envp.read_cstring_array()?;
        if args.is_empty() {
            error!("execve: args is null");
            return Err(LxError::EINVAL);
        }

        // TODO: check and kill other threads

        // Read program file
        let proc = self.linux_process();
        let inode = proc.lookup_inode(&path)?;
        let data = inode.read_as_vec()?;

        let vmar = self.zircon_process().vmar();
        vmar.clear()?;
        let loader = LinuxElfLoader {
            syscall_entry: self.syscall_entry,
            stack_pages: 8,
            root_inode: proc.root_inode().clone(),
        };
        let (entry, sp) = loader.load(&vmar, &data, args, envs)?;

        // Modify exec path
        proc.set_execute_path(&path);

        // TODO: use right signal
        self.zircon_process().signal_set(Signal::SIGNALED);

        *self.context = UserContext::new_fn(entry, sp, 0, 0);
        Ok(0)
    }
    //
    //    pub fn sys_yield(&self) -> SysResult {
    //        thread::yield_now();
    //        Ok(0)
    //    }
    //
    //    /// Kill the process
    //    pub fn sys_kill(&self, pid: usize, sig: usize) -> SysResult {
    //        info!(
    //            "kill: thread {} kill process {} with signal {}",
    //            thread::current().id(),
    //            pid,
    //            sig
    //        );
    //        let current_pid = self.process().pid.get().clone();
    //        if current_pid == pid {
    //            // killing myself
    //            self.sys_exit_group(sig);
    //        } else {
    //            if let Some(proc_arc) = PROCESSES.read().get(&pid).and_then(|weak| weak.upgrade()) {
    //                let mut proc = proc_arc.lock();
    //                proc.exit(sig);
    //                Ok(0)
    //            } else {
    //                Err(LxError::EINVAL)
    //            }
    //        }
    //    }

    /// Get the current thread ID.
    pub fn sys_gettid(&self) -> SysResult {
        info!("gettid:");
        let tid = self.thread.id();
        Ok(tid as usize)
    }

    /// Get the current process ID.
    pub fn sys_getpid(&self) -> SysResult {
        info!("getpid:");
        let proc = self.zircon_process();
        let pid = proc.id();
        Ok(pid as usize)
    }

    /// Get the parent process ID.
    pub fn sys_getppid(&self) -> SysResult {
        info!("getppid:");
        let proc = self.linux_process();
        let ppid = proc.parent().map(|p| p.id()).unwrap_or(0);
        Ok(ppid as usize)
    }

    /// Exit the current thread
    pub fn sys_exit(&mut self, exit_code: i32) -> SysResult {
        info!("exit: code={}", exit_code);
        self.thread.exit_linux(exit_code);
        self.exit = true;
        Err(LxError::ENOSYS)
    }

    /// Exit the current thread group (i.e. process)
    pub fn sys_exit_group(&mut self, exit_code: i32) -> SysResult {
        info!("exit_group: code={}", exit_code);
        let proc = self.zircon_process();
        proc.exit(exit_code as i64);
        self.exit = true;
        Err(LxError::ENOSYS)
    }

    //    pub fn sys_nanosleep(&self, req: *const TimeSpec) -> SysResult {
    //        let time = unsafe { *self.vm().check_read_ptr(req)? };
    //        info!("nanosleep: time: {:#?}", time);
    //        // TODO: handle spurious wakeup
    //        thread::sleep(time.to_duration());
    //        Ok(0)
    //    }
    //
    //    pub fn sys_set_priority(&self, priority: usize) -> SysResult {
    //        let pid = thread::current().id();
    //        thread_manager().set_priority(pid, priority as u8);
    //        Ok(0)
    //    }

    pub fn sys_set_tid_address(&self, tidptr: UserOutPtr<i32>) -> SysResult {
        info!("set_tid_address: {:?}", tidptr);
        self.thread.set_tid_address(tidptr);
        let tid = self.thread.id();
        Ok(tid as usize)
    }
}

bitflags! {
    pub struct CloneFlags: usize {
        const CSIGNAL =         0xff;
        const VM =              1 << 8;
        const FS =              1 << 9;
        const FILES =           1 << 10;
        const SIGHAND =         1 << 11;
        const PTRACE =          1 << 13;
        const VFORK =           1 << 14;
        const PARENT =          1 << 15;
        const THREAD =          1 << 16;
        const NEWNS	=           1 << 17;
        const SYSVSEM =         1 << 18;
        const SETTLS =          1 << 19;
        const PARENT_SETTID =   1 << 20;
        const CHILD_CLEARTID =  1 << 21;
        const DETACHED =        1 << 22;
        const UNTRACED =        1 << 23;
        const CHILD_SETTID =    1 << 24;
        const NEWCGROUP =       1 << 25;
        const NEWUTS =          1 << 26;
        const NEWIPC =          1 << 27;
        const NEWUSER =         1 << 28;
        const NEWPID =          1 << 29;
        const NEWNET =          1 << 30;
        const IO =              1 << 31;
    }
}

trait CtxExt {
    fn new_fn(entry: usize, sp: usize, arg1: usize, arg2: usize) -> Self;
    fn new_clone(regs: &Self, newsp: usize, newtls: usize) -> Self;
    fn new_fork(regs: &Self) -> Self;
}

impl CtxExt for UserContext {
    fn new_fn(entry: usize, sp: usize, arg1: usize, arg2: usize) -> Self {
        let mut ctx = UserContext::default();
        ctx.set_ip(entry);
        ctx.set_sp(sp);
        ctx.set_syscall_args([arg1, arg2, 0, 0, 0, 0]);
        ctx
    }

    fn new_clone(origin_ctx: &Self, newsp: usize, newtls: usize) -> Self {
        let mut ctx = UserContext::default();
        ctx.general = origin_ctx.general;
        ctx.set_syscall_ret(0);
        ctx.set_sp(newsp);
        ctx.set_tls(newtls);
        ctx
    }

    fn new_fork(origin_ctx: &Self) -> Self {
        let mut ctx = UserContext::default();
        ctx.general = origin_ctx.general;
        ctx.set_syscall_ret(0);
        ctx
    }
}
