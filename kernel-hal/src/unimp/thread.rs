use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;

/// Spawn a new thread.
pub fn spawn(_future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>, _vmtoken: usize) {
    unimplemented!()
}

/// Set tid and pid of current task.
pub fn set_tid(_tid: u64, _pid: u64) {
    unimplemented!()
}

/// Get tid and pid of current task.]
pub fn get_tid() -> (u64, u64) {
    unimplemented!()
}
