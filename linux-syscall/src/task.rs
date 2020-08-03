//! Syscalls for process
//!
//! - fork
//! - vfork
//! - clone
//! - wait4
//! - execve
//! - gettid
//! - getpid
//! - getppid

use super::*;
use bitflags::bitflags;
use core::fmt::Debug;
use linux_object::fs::INodeExt;
use linux_object::loader::LinuxElfLoader;
use linux_object::thread::{CurrentThreadExt, ThreadExt};
use linux_object::time::*;

impl Syscall<'_> {
    /// Fork the current process. Return the child's PID.
    pub fn sys_fork(&self) -> SysResult {
        info!("fork:");
        let new_proc = Process::fork_from(self.zircon_process(), false)?;
        let new_thread = Thread::create_linux(&new_proc)?;
        new_thread.start_with_regs(GeneralRegs::new_fork(self.regs), self.thread_fn)?;

        info!("fork: {} -> {}", self.zircon_process().id(), new_proc.id());
        Ok(new_proc.id() as usize)
    }

    /// creates a child process of the calling process, similar to fork but wait for execve
    pub async fn sys_vfork(&self) -> SysResult {
        info!("vfork:");
        let new_proc = Process::fork_from(self.zircon_process(), true)?;
        let new_thread = Thread::create_linux(&new_proc)?;
        new_thread.start_with_regs(GeneralRegs::new_fork(self.regs), self.thread_fn)?;

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
        let _flags = CloneFlags::from_bits(flags).ok_or(LxError::EINVAL)?;
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
        let regs = GeneralRegs::new_clone(self.regs, newsp, newtls);
        new_thread.start_with_regs(regs, self.thread_fn)?;

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
        let flags = WaitFlags::from_bits(options).ok_or(LxError::EINVAL)?;
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
        let path = path.read_cstring()?;
        let args = argv.read_cstring_array()?;
        let envs = envp.read_cstring_array()?;
        info!(
            "execve: path: {:?}, argv: {:?}, envs: {:?}",
            path, argv, envs
        );
        if args.is_empty() {
            error!("execve: args is null");
            return Err(LxError::EINVAL);
        }

        // TODO: check and kill other threads

        // Read program file
        let proc = self.linux_process();
        let inode = proc.lookup_inode(&path)?;
        let data = inode.read_as_vec()?;

        proc.remove_cloexec_files();

        let vmar = self.zircon_process().vmar();
        vmar.clear()?;
        let loader = LinuxElfLoader {
            syscall_entry: self.syscall_entry,
            stack_pages: 8,
            root_inode: proc.root_inode().clone(),
        };
        let (entry, sp) = loader.load(&vmar, &data, args, envs, path.clone())?;

        // Modify exec path
        proc.set_execute_path(&path);

        // TODO: use right signal
        self.zircon_process().signal_set(Signal::SIGNALED);

        *self.regs = GeneralRegs::new_fn(entry, sp, 0, 0);
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
        Err(LxError::ENOSYS)
    }

    /// Exit the current thread group (i.e. process)
    pub fn sys_exit_group(&mut self, exit_code: i32) -> SysResult {
        info!("exit_group: code={}", exit_code);
        let proc = self.zircon_process();
        proc.exit(exit_code as i64);
        Err(LxError::ENOSYS)
    }

    /// Allows the calling thread to sleep for
    /// an interval specified with nanosecond precision
    pub async fn sys_nanosleep(&self, req: UserInPtr<TimeSpec>) -> SysResult {
        info!("nanosleep: deadline={:?}", req);
        let req = req.read()?;
        kernel_hal::sleep_until(req.into()).await;
        Ok(0)
    }

    //    pub fn sys_set_priority(&self, priority: usize) -> SysResult {
    //        let pid = thread::current().id();
    //        thread_manager().set_priority(pid, priority as u8);
    //        Ok(0)
    //    }

    /// set pointer to thread ID
    /// returns the caller's thread ID
    pub fn sys_set_tid_address(&self, tidptr: UserOutPtr<i32>) -> SysResult {
        info!("set_tid_address: {:?}", tidptr);
        self.thread.set_tid_address(tidptr);
        let tid = self.thread.id();
        Ok(tid as usize)
    }
}

bitflags! {
    pub struct CloneFlags: usize {
        ///
        const CSIGNAL =         0xff;
        /// the calling process and the child process run in the same memory space
        const VM =              1 << 8;
        /// the caller and the child process share the same filesystem information
        const FS =              1 << 9;
        /// the calling process and the child process share the same file descriptor table
        const FILES =           1 << 10;
        /// the calling process and the child process share the same table of signal handlers.
        const SIGHAND =         1 << 11;
        /// the calling process is being traced
        const PTRACE =          1 << 13;
        /// the execution of the calling process is suspended until the child releases its virtual memory resources
        const VFORK =           1 << 14;
        /// the parent of the new child will be the same as that of the call‐ing process.
        const PARENT =          1 << 15;
        /// the child is placed in the same thread group as the calling process.
        const THREAD =          1 << 16;
        /// cloned child is started in a new mount namespace
        const NEWNS	=           1 << 17;
        /// the child and the calling process share a single list of System V semaphore adjustment values.
        const SYSVSEM =         1 << 18;
        /// architecture dependent, The TLS (Thread Local Storage) descriptor is set to tls.
        const SETTLS =          1 << 19;
        /// Store the child thread ID at the location in the parent's memory.
        const PARENT_SETTID =   1 << 20;
        /// Clear (zero) the child thread ID
        const CHILD_CLEARTID =  1 << 21;
        /// the parent not to receive a signal when the child terminated
        const DETACHED =        1 << 22;
        /// a tracing process cannot force CLONE_PTRACE on this child process.
        const UNTRACED =        1 << 23;
        /// Store the child thread ID
        const CHILD_SETTID =    1 << 24;
        /// Create the process in a new cgroup namespace.
        const NEWCGROUP =       1 << 25;
        /// create the process in a new UTS namespace
        const NEWUTS =          1 << 26;
        /// create the process in a new IPC namespace.
        const NEWIPC =          1 << 27;
        /// create the process in a new user namespace
        const NEWUSER =         1 << 28;
        /// create the process in a new PID namespace
        const NEWPID =          1 << 29;
        /// create the process in a new net‐work namespace.
        const NEWNET =          1 << 30;
        /// the new process shares an I/O context with the calling process.
        const IO =              1 << 31;
    }
}

trait RegExt {
    fn new_fn(entry: usize, sp: usize, arg1: usize, arg2: usize) -> Self;
    fn new_clone(regs: &Self, newsp: usize, newtls: usize) -> Self;
    fn new_fork(regs: &Self) -> Self;
}

#[cfg(target_arch = "x86_64")]
impl RegExt for GeneralRegs {
    fn new_fn(entry: usize, sp: usize, arg1: usize, arg2: usize) -> Self {
        GeneralRegs {
            rip: entry,
            rsp: sp,
            rdi: arg1,
            rsi: arg2,
            ..Default::default()
        }
    }

    fn new_clone(regs: &Self, newsp: usize, newtls: usize) -> Self {
        GeneralRegs {
            rax: 0,
            rsp: newsp,
            fsbase: newtls,
            ..*regs
        }
    }

    fn new_fork(regs: &Self) -> Self {
        GeneralRegs { rax: 0, ..*regs }
    }
}
