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
use num::FromPrimitive;

impl Syscall<'_> {
    /// Used to change the action taken by a process on receipt of a specific signal.
    pub fn sys_rt_sigaction(
        &self,
        signum: usize,
        act: UserInPtr<SignalAction>,
        mut oldact: UserOutPtr<SignalAction>,
        sigsetsize: usize,
    ) -> SysResult {
        if let Some(signal) = <Signal as FromPrimitive>::from_usize(signum) {
            info!(
                "rt_sigaction: signum: {:?}, act: {:?}, oldact: {:?}, sigsetsize: {}",
                signal, act, oldact, sigsetsize
            );
            use Signal::*;
            if signal == SIGKILL
                || signal == SIGSTOP
                || sigsetsize != core::mem::size_of::<Sigset>()
            {
                Err(LxError::EINVAL)
            } else {
                let mut sinner = self.linux_process().signal_inner();
                if !oldact.is_null() {
                    oldact.write(sinner.dispositions[signum])?;
                }
                if !act.is_null() {
                    let act = act.read()?;
                    info!("new action: {:?} -> {:x?}", signal, act);
                    sinner.dispositions[signum] = act;
                }
                Ok(0)
            }
        } else {
            info!(
                "rt_sigaction: signal: UNKNOWN, act: {:?}, oldact: {:?}, sigsetsize: {}",
                act, oldact, sigsetsize
            );
            Err(LxError::EINVAL)
        }
    }

    /// Used to fetch and/or change the signal mask of the calling thread
    pub fn sys_rt_sigprocmask(
        &mut self,
        how: usize,
        set: UserInPtr<Sigset>,
        mut oldset: UserOutPtr<Sigset>,
        sigsetsize: usize,
    ) -> SysResult {
        info!(
            "rt_sigprocmask: how: {}, set: {:?}, oldset: {:?}, sigsetsize: {}",
            how, set, oldset, sigsetsize
        );
        if sigsetsize != 8 {
            return Err(LxError::EINVAL);
        }
        if !oldset.is_null() {
            oldset.write(self.thread.lock_linux().signal_inner().sig_mask)?;
        }
        if !set.is_null() {
            let set = set.read()?;
            const BLOCK: usize = 0;
            const UNBLOCK: usize = 1;
            const SETMASK: usize = 2;
            let thread = self.thread.lock_linux();
            let mut inner = thread.signal_inner();
            match how {
                BLOCK => {
                    info!("rt_sigprocmask: block: {:x?}", set);
                    inner.sig_mask.add_set(&set);
                }
                UNBLOCK => {
                    info!("rt_sigprocmask: unblock: {:x?}", set);
                    inner.sig_mask.remove_set(&set)
                }
                SETMASK => {
                    info!("rt_sigprocmask: set: {:x?}", set);
                    inner.sig_mask = set;
                }
                _ => return Err(LxError::EINVAL),
            }
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
        info!("sigaltstack: ss: {:?}, old_ss: {:?}", ss, old_ss);
        if !old_ss.is_null() {
            old_ss.write(
                self.thread
                    .lock_linux()
                    .signal_inner()
                    .signal_alternate_stack,
            )?;
        }
        if !ss.is_null() {
            let ss = ss.read()?;
            info!("new stack: {:?}", ss);

            // check stack size when not disable
            const MINSIGSTKSZ: usize = 2048;
            if ss.flags & SignalStackFlags::DISABLE.bits() != 0 && ss.size < MINSIGSTKSZ {
                return Err(LxError::ENOMEM);
            }

            // only allow SS_AUTODISARM and SS_DISABLE
            if ss.flags
                != ss.flags
                    & (SignalStackFlags::AUTODISARM.bits() | SignalStackFlags::DISABLE.bits())
            {
                return Err(LxError::EINVAL);
            }

            let thread = self.thread.lock_linux();
            let mut inner = thread.signal_inner();
            let old_ss = &mut inner.signal_alternate_stack;
            let flags = SignalStackFlags::from_bits_truncate(old_ss.flags);
            if flags.contains(SignalStackFlags::ONSTACK) {
                // cannot change signal alternate stack when we are on it
                // see man sigaltstack(2)
                return Err(LxError::EPERM);
            }
            *old_ss = ss;
        }
        Ok(0)
    }
}
