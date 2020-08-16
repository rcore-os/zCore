//! Syscalls of signal
//!
//! - rt_sigaction
//! - rt_sigreturn
//! - rt_sigprocmask
//! - kill
//! - tkill
//! - sigaltstack

use super::*;
use linux_object::signal::{Signal, SignalAction, SignalStack, SignalStackFlags, Sigset};
use linux_object::thread::ThreadExt;
use numeric_enum_macro::numeric_enum;

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
            "rt_sigaction: signal={:?}, act={:?}, oldact={:?}, sigsetsize={}",
            signal, act, oldact, sigsetsize
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
            "rt_sigprocmask: how={:?}, set={:?}, oldset={:?}, sigsetsize={}",
            how, set, oldset, sigsetsize
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
}
