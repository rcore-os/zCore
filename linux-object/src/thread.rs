//! Linux Thread

use crate::process::ProcessExt;
use alloc::sync::Arc;
use kernel_hal::user::{Out, UserOutPtr, UserPtr};
use kernel_hal::VirtAddr;
use spin::{Mutex, MutexGuard};
use zircon_object::task::{Process, Thread};
use zircon_object::ZxResult;

pub trait ThreadExt {
    fn create_linux(proc: &Arc<Process>) -> ZxResult<Arc<Self>>;
    fn lock_linux(&self) -> MutexGuard<'_, LinuxThread>;
    fn set_tid_address(&self, tidptr: UserOutPtr<i32>);
    fn exit_linux(&self, exit_code: i32);
}

impl ThreadExt for Thread {
    fn create_linux(proc: &Arc<Process>) -> ZxResult<Arc<Self>> {
        let linux_thread = Mutex::new(LinuxThread {
            clear_child_tid: 0.into(),
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
            let futex = self.proc().lock_linux().get_futex(uaddr);
            futex.wake(1);
        }
        self.exit();
    }
}

/// Linux specific thread information.
pub struct LinuxThread {
    /// Kernel performs futex wake when thread exits.
    /// Ref: [http://man7.org/linux/man-pages/man2/set_tid_address.2.html]
    clear_child_tid: UserOutPtr<i32>,
}
