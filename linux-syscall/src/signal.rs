//! Syscalls of signal
//!
//! - rt_sigaction
//! - rt_sigreturn
//! - rt_sigprocmask
//! - rt_sigtimedwait
//! - kill
//! - tkill
//! - sigaltstack

use super::*;
use linux_object::signal::{SigInfo, Signal, SignalAction, SignalStack, SignalStackFlags, Sigset};
use linux_object::thread::ThreadExt;
use linux_object::time::TimeSpec;
use numeric_enum_macro::numeric_enum;
use zircon_object::object::KernelObject;
use zircon_object::task::ROOT_JOB;

impl Syscall<'_> {
    /// Used to change the action taken by a process on receipt of a specific signal.
    pub fn sys_rt_sigaction(
        &self,
        signum: usize,
        act: UserInPtr<SignalAction>,
        mut oldact: UserOutPtr<SignalAction>,
        sigsetsize: usize,
    ) -> SysResult {
        let signal = Signal::try_from(signum as u8).map_err(|_| LxError::EINVAL)?;
        info!(
            "rt_sigaction: signal={:?}, act={:?}, oldact={:?}, sigsetsize={}, thread={}",
            signal,
            act,
            oldact,
            sigsetsize,
            self.thread.id()
        );
        if sigsetsize != core::mem::size_of::<Sigset>()
            || signal == Signal::SIGKILL
            || signal == Signal::SIGSTOP
        {
            return Err(LxError::EINVAL);
        }
        let proc = self.linux_process();
        oldact.write_if_not_null(proc.signal_action(signal))?;
        if let Some(act) = act.read_if_not_null()? {
            info!("new action: {:?} -> {:x?}", signal, act);
            proc.set_signal_action(signal, act);
        }
        Ok(0)
    }

    /// Used to fetch and/or change the signal mask of the calling thread
    pub fn sys_rt_sigprocmask(
        &mut self,
        how: i32,
        set: UserInPtr<Sigset>,
        mut oldset: UserOutPtr<Sigset>,
        sigsetsize: usize,
    ) -> SysResult {
        numeric_enum! {
            #[repr(i32)]
            #[derive(Debug)]
            enum How {
                Block = 0,
                Unblock = 1,
                SetMask = 2,
            }
        }
        let how = How::try_from(how).map_err(|_| LxError::EINVAL)?;
        info!(
            "rt_sigprocmask: how={:?}, set={:?}, oldset={:?}, sigsetsize={}, thread={}",
            how,
            set,
            oldset,
            sigsetsize,
            self.thread.id()
        );
        if sigsetsize != core::mem::size_of::<Sigset>() {
            return Err(LxError::EINVAL);
        }
        oldset.write_if_not_null(self.thread.lock_linux().signal_mask)?;
        if set.is_null() {
            return Ok(0);
        }
        let set = set.read()?;
        let mut thread = self.thread.lock_linux();
        match how {
            How::Block => thread.signal_mask.insert_set(&set),
            How::Unblock => thread.signal_mask.remove_set(&set),
            How::SetMask => thread.signal_mask = set,
        }
        Ok(0)
    }

    /// Allows a process to define a new alternate signal stack
    /// and/or retrieve the state of an existing alternate signal stack
    pub fn sys_sigaltstack(
        &self,
        ss: UserInPtr<SignalStack>,
        mut old_ss: UserOutPtr<SignalStack>,
    ) -> SysResult {
        info!("sigaltstack: ss={:?}, old_ss={:?}", ss, old_ss);
        let mut thread = self.thread.lock_linux();
        old_ss.write_if_not_null(thread.signal_alternate_stack)?;
        if ss.is_null() {
            return Ok(0);
        }
        let ss = ss.read()?;
        // check stack size when not disable
        const MIN_SIGSTACK_SIZE: usize = 2048;
        if ss.flags.contains(SignalStackFlags::DISABLE) && ss.size < MIN_SIGSTACK_SIZE {
            return Err(LxError::ENOMEM);
        }
        // only allow SS_AUTODISARM and SS_DISABLE
        if !(SignalStackFlags::AUTODISARM | SignalStackFlags::DISABLE).contains(ss.flags) {
            return Err(LxError::EINVAL);
        }
        let old_ss = &mut thread.signal_alternate_stack;
        if old_ss.flags.contains(SignalStackFlags::ONSTACK) {
            // cannot change signal alternate stack when we are on it
            // see man sigaltstack(2)
            return Err(LxError::EPERM);
        }
        *old_ss = ss;
        Ok(0)
    }

    /// Send a signal to a process specified by pid
    /// TODO: support all the arguments
    pub fn sys_kill(&self, pid: isize, signum: usize) -> SysResult {
        let signal = Signal::try_from(signum as u8).map_err(|_| LxError::EINVAL)?;
        info!(
            "kill: thread {} kill process {} with signal {:?}",
            self.thread.id(),
            pid,
            signal
        );
        // NOTE: process-group sends use a minimal "pgid == leader pid" model and
        // a signal is delivered to one not-blocking thread of the target process
        // (see `send_to_pid`). This is sufficient for the shells/job-control we
        // run; it is intentionally not a full POSIX implementation. (Previously a
        // warn! fired on every kill() to say so, which only spammed the log.)
        enum SendTarget {
            EveryProcessInGroup,
            EveryProcess,
            EveryProcessInGroupByPID(KoID),
            Pid(KoID),
        }
        let target = match pid {
            p if p > 0 => SendTarget::Pid(p as KoID),
            0 => SendTarget::EveryProcessInGroup,
            -1 => SendTarget::EveryProcess,
            p if p < -1 => SendTarget::EveryProcessInGroupByPID((-p) as KoID),
            _ => unimplemented!(),
        };
        let caller = self.zircon_process().clone();
        let send_to_pid = |pid: KoID| -> SysResult {
            let process = ROOT_JOB.find_process(pid).ok_or(LxError::ESRCH)?;
            match signal {
                Signal::SIGKILL => {
                    let retcode = (128 + Signal::SIGKILL as i32) as i64;
                    if caller.id() == process.id() {
                        caller.exit(retcode);
                    } else {
                        process.exit(retcode);
                    }
                }
                sig => {
                    let tids = process.thread_ids();
                    for tid in tids {
                        let thread = process.get_child(tid).unwrap();
                        let thread: Arc<Thread> = thread.downcast_arc().unwrap();
                        let mut thread_linux = thread.lock_linux();
                        if thread_linux.signal_mask.contains(sig) {
                            continue;
                        } else {
                            thread_linux.signals.insert(signal);
                            break;
                        }
                    }
                }
            };
            Ok(0)
        };
        match target {
            SendTarget::Pid(pid) => send_to_pid(pid),
            SendTarget::EveryProcessInGroup => {
                // Minimal process-group support: without a real setpgid/pgid table,
                // treat "current process group" as the current process ID.
                send_to_pid(caller.id() as KoID)
            }
            SendTarget::EveryProcessInGroupByPID(pgid) => {
                // Minimal process-group support: treat pgid as the leader's pid.
                // This matches the common shell behavior of setting fg_pgrp = child pid.
                send_to_pid(pgid)
            }
            SendTarget::EveryProcess => match signal {
                Signal::SIGKILL => {
                    for proc in linux_object::process::all_live_processes() {
                        let retcode = (128 + Signal::SIGKILL as i32) as i64;
                        if caller.id() == proc.id() {
                            caller.exit(retcode);
                        } else {
                            proc.exit(retcode);
                        }
                    }
                    Ok(0)
                }
                sig => linux_object::process::send_signal_to_all_processes(sig).map(|_| 0),
            },
        }
    }

    /// Send a signal to a thread specified by tid
    pub fn sys_tkill(&mut self, tid: usize, signum: usize) -> SysResult {
        let signal = Signal::try_from(signum as u8).map_err(|_| LxError::EINVAL)?;
        info!(
            "tkill: thread {} kill thread {} with signal {:?}",
            self.thread.id(),
            tid,
            signum
        );
        let parent = self.zircon_process().clone();
        match parent.get_child(tid as u64) {
            Ok(obj) => {
                let thread: Arc<Thread> = obj.downcast_arc().unwrap();
                let mut thread_linux = thread.lock_linux();
                thread_linux.signals.insert(signal);
                drop(thread_linux);
                Ok(0)
            }
            Err(_) => Err(LxError::EINVAL),
        }
    }

    /// Send a signal to a thread specified by tgid (i.e., process) and pid
    /// Note: the job of the target process should be the same as the calling thread
    pub fn sys_tgkill(&mut self, tgid: usize, tid: usize, signum: usize) -> SysResult {
        let signal = Signal::try_from(signum as u8).map_err(|_| LxError::EINVAL)?;
        info!(
            "tkill: thread {} kill thread {} in process {} with signal {:?}",
            self.thread.id(),
            tid,
            tgid,
            signum
        );
        warn!(
            "The signal will be delivered to the target process that 
            belongs to the same job as the calling thread."
        );
        let parent = self.zircon_process().clone();
        match parent
            .job()
            .get_child(tgid as u64)
            .map(|proc| proc.get_child(tid as u64))
        {
            Ok(Ok(obj)) => {
                let thread: Arc<Thread> = obj.downcast_arc().unwrap();
                let mut thread_linux = thread.lock_linux();
                thread_linux.signals.insert(signal);
                drop(thread_linux);
                Ok(0)
            }
            _ => Err(LxError::EINVAL),
        }
    }

    /// Return from handling some signal
    pub fn sys_rt_sigreturn(&mut self) -> SysResult {
        info!(
            "sigreturn: thread {} returns from handling the signal",
            self.thread.id()
        );
        let (old_ctx, siginfo_ptr, uctx_ptr) = self.thread.fetch_backup_context().unwrap();
        self.thread
            .with_context(|ctx| {
                self.thread.lock_linux().restore_after_handle_signal(
                    ctx,
                    &old_ctx,
                    siginfo_ptr,
                    uctx_ptr,
                )
            })
            .unwrap();
        // sigreturn has no return value of its own: the generic syscall-return
        // path writes our result into rax AFTER the context restore above, so
        // returning a fixed 0 would clobber the interrupted syscall's restored
        // result. Observed: a SIGWINCH handler returning into a just-completed
        // read(2) turned its rax=1 into 0 — busybox lineedit took the 0 as
        // EOF and the shell exited the moment foot resized its window. Return
        // the restored rax so the generic write-back is a no-op.
        let rax = self
            .thread
            .with_context(|ctx| ctx.get_field(kernel_hal::context::UserContextField::ReturnValue))
            .unwrap();
        Ok(rax)
    }

    /// Temporarily replace the signal mask of the calling thread with `mask`
    /// and suspend the thread until a signal is delivered whose action is to
    /// invoke a handler or to terminate the process.
    ///
    /// Always returns `-EINTR` once a signal is delivered. The original mask is
    /// restored after the handler returns, via the saved-mask mechanism in
    /// `LinuxThread` (see `handle_signal`).
    pub async fn sys_rt_sigsuspend(
        &mut self,
        mask: UserInPtr<Sigset>,
        sigsetsize: usize,
    ) -> SysResult {
        if sigsetsize != core::mem::size_of::<Sigset>() {
            return Err(LxError::EINVAL);
        }
        let mut newmask = mask.read()?;
        // SIGKILL and SIGSTOP can never be blocked.
        newmask.remove(Signal::SIGKILL);
        newmask.remove(Signal::SIGSTOP);
        info!(
            "rt_sigsuspend: mask={:#x}, thread={}",
            newmask.val(),
            self.thread.id()
        );
        // Install the temporary mask and remember the previous one so it is
        // restored once the awakening signal handler returns.
        {
            let mut thread = self.thread.lock_linux();
            let old_mask = thread.signal_mask;
            thread.signal_mask = newmask;
            thread.saved_sigmask = Some(old_mask);
        }
        // Block until a signal becomes deliverable under the temporary mask
        // (or the thread/process is being torn down). `check_signals` reports
        // this as `EINTR`, which is exactly the return value `sigsuspend` owes
        // its caller.
        loop {
            if let Err(e) = linux_object::process::check_signals() {
                return Err(e);
            }
            let deadline = kernel_hal::timer::deadline_after(core::time::Duration::from_millis(10));
            kernel_hal::thread::sleep_until(deadline).await;
        }
    }

    /// Suspend the calling thread until a signal is delivered that either
    /// terminates the process or causes a signal handler to be invoked.
    ///
    /// Always returns `-EINTR`.
    pub async fn sys_pause(&mut self) -> SysResult {
        info!("pause: thread {}", self.thread.id());
        loop {
            if let Err(e) = linux_object::process::check_signals() {
                return Err(e);
            }
            let deadline = kernel_hal::timer::deadline_after(core::time::Duration::from_millis(10));
            kernel_hal::thread::sleep_until(deadline).await;
        }
    }

    /// Synchronously wait for one of the signals in `set` to become pending,
    /// dequeue it, and return its number — *without* invoking its handler.
    ///
    /// The caller is expected (POSIX) to have blocked the signals in `set`; a
    /// blocked signal is still queued to the thread's pending set when sent, and
    /// this call is what consumes it. Returns the signal number on success,
    /// `-EAGAIN` if `timeout` elapses with nothing delivered, and `-EINTR` if an
    /// unblocked signal *outside* `set` interrupts the wait.
    ///
    /// busybox `init` (PID 1) parks here between reaping children; while this
    /// was unimplemented it spun, flooding the log with
    /// `unknown syscall: RT_SIGTIMEDWAIT`.
    pub async fn sys_rt_sigtimedwait(
        &mut self,
        set: UserInPtr<Sigset>,
        mut info: UserOutPtr<SigInfo>,
        timeout: UserInPtr<TimeSpec>,
        sigsetsize: usize,
    ) -> SysResult {
        if sigsetsize != core::mem::size_of::<Sigset>() {
            return Err(LxError::EINVAL);
        }
        let mut waitset = set.read()?;
        // SIGKILL/SIGSTOP can never be caught or waited for.
        waitset.remove(Signal::SIGKILL);
        waitset.remove(Signal::SIGSTOP);
        // A null `timeout` means wait indefinitely; otherwise resolve the
        // monotonic deadline once, up front.
        let deadline = if timeout.is_null() {
            None
        } else {
            let dur = core::time::Duration::from(timeout.read()?);
            Some(kernel_hal::timer::timer_now() + dur)
        };
        info!(
            "rt_sigtimedwait: set={:#x}, timeout={:?}, thread={}",
            waitset.val(),
            deadline,
            self.thread.id()
        );
        loop {
            // A waited-for signal already pending? Dequeue and return it. We do
            // not have per-signal queued `siginfo` (pending signals are a plain
            // bitmask), so only `si_signo` is reported — enough for callers like
            // busybox init that follow up with `waitpid`.
            {
                let mut thread = self.thread.lock_linux();
                let ready = Sigset::new(thread.signals.val() & waitset.val());
                if let Some(sig) = ready.find_first_signal() {
                    thread.signals.remove(sig);
                    drop(thread);
                    if !info.is_null() {
                        let mut si = SigInfo::default();
                        si.signo = sig as i32;
                        info.write(si)?;
                    }
                    return Ok(sig as usize);
                }
            }
            // An unblocked signal *outside* `set` interrupts the wait (EINTR).
            linux_object::process::check_signals()?;
            // Timed out with nothing delivered.
            if let Some(end) = deadline {
                if kernel_hal::timer::timer_now() >= end {
                    return Err(LxError::EAGAIN);
                }
            }
            let next = kernel_hal::timer::deadline_after(core::time::Duration::from_millis(10));
            kernel_hal::thread::sleep_until(next).await;
        }
    }
}
