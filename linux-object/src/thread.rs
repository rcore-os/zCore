//! Linux Thread

use crate::process::ProcessExt;
use crate::signal::{SignalStack, Sigset};
use alloc::sync::Arc;
use kernel_hal::user::{Out, UserOutPtr, UserPtr};
use kernel_hal::VirtAddr;
use spin::{Mutex, MutexGuard};
use zircon_object::task::{CurrentThread, Process, Thread};
use zircon_object::ZxResult;

/// Thread extension for linux
pub trait ThreadExt {
    /// create linux thread
    fn create_linux(proc: &Arc<Process>) -> ZxResult<Arc<Self>>;
    /// lock and get Linux thread
    fn lock_linux(&self) -> MutexGuard<'_, LinuxThread>;
    /// Set pointer to thread ID.
    fn set_tid_address(&self, tidptr: UserOutPtr<i32>);
}

/// CurrentThread extension for linux
pub trait CurrentThreadExt {
    /// exit linux thread
    fn exit_linux(&self, exit_code: i32);
}

impl ThreadExt for Thread {
    fn create_linux(proc: &Arc<Process>) -> ZxResult<Arc<Self>> {
        let linux_thread = Mutex::new(LinuxThread {
            clear_child_tid: 0.into(),
            signal_mask: Sigset::default(),
            signal_alternate_stack: SignalStack::default(),
        });
        Thread::create_with_ext(proc, "", linux_thread)
    }

    fn lock_linux(&self) -> MutexGuard<'_, LinuxThread> {
        self.ext()
            .downcast_ref::<Mutex<LinuxThread>>()
            .unwrap()
            .lock()
    }

    /// Set pointer to thread ID.
    fn set_tid_address(&self, tidptr: UserPtr<i32, Out>) {
        self.lock_linux().clear_child_tid = tidptr;
    }
}

impl CurrentThreadExt for CurrentThread {
    /// Exit current thread for Linux.
    fn exit_linux(&self, _exit_code: i32) {
        let mut linux_thread = self.lock_linux();
        let clear_child_tid = &mut linux_thread.clear_child_tid;
        // perform futex wake 1
        // ref: http://man7.org/linux/man-pages/man2/set_tid_address.2.html
        if !clear_child_tid.is_null() {
            info!("exit: do futex {:?} wake 1", clear_child_tid);
            clear_child_tid.write(0).unwrap();
            let uaddr = clear_child_tid.as_ptr() as VirtAddr;
            let futex = self.proc().linux().get_futex(uaddr);
            futex.wake(1);
        }
        self.exit();
    }
}

/// Linux specific thread information.
pub struct LinuxThread {
    /// Kernel performs futex wake when thread exits.
    /// Ref: <http://man7.org/linux/man-pages/man2/set_tid_address.2.html>
    clear_child_tid: UserOutPtr<i32>,
    /// Signal mask
    pub signal_mask: Sigset,
    /// signal alternate stack
    pub signal_alternate_stack: SignalStack,
}
