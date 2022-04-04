//! Linux Thread

use crate::error::SysResult;
use crate::process::ProcessExt;
use crate::signal::{SignalStack, Sigset};
use alloc::sync::Arc;
use kernel_hal::user::{Out, UserInPtr, UserOutPtr, UserPtr};
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
    /// Get robust list.
    fn get_robust_list(
        &self,
        _head_ptr: UserOutPtr<UserOutPtr<RobustList>>,
        _len_ptr: UserOutPtr<usize>,
    ) -> SysResult;
    /// Set robust list.
    fn set_robust_list(&self, head: UserInPtr<RobustList>, len: usize);
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
            robust_list: 0.into(),
            robust_list_len: 0,
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

    fn get_robust_list(
        &self,
        mut _head_ptr: UserOutPtr<UserOutPtr<RobustList>>,
        mut _len_ptr: UserOutPtr<usize>,
    ) -> SysResult {
        _head_ptr = (self.lock_linux().robust_list.as_ptr() as *mut RobustList as usize).into();
        _len_ptr = (&self.lock_linux().robust_list_len as *const usize as usize).into();
        Ok(0)
    }

    fn set_robust_list(&self, head: UserInPtr<RobustList>, len: usize) {
        self.lock_linux().robust_list = head;
        self.lock_linux().robust_list_len = len;
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

/// robust_list
#[derive(Default)]
pub struct RobustList {
    /// head
    pub head: usize,
    /// off
    pub off: isize,
    /// pending
    pub pending: usize,
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
    /// robust_list
    robust_list: UserInPtr<RobustList>,
    robust_list_len: usize,
}
