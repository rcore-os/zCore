//! Thread spawning.

use alloc::sync::Arc;
use async_std::task_local;
use core::{any::Any, cell::RefCell, future::Future};

task_local! {
    static CURRENT_THREAD: RefCell<Option<Arc<dyn Any + Send + Sync>>> = RefCell::new(None);
}

hal_fn_impl! {
    impl mod crate::hal_fn::thread {
        fn spawn(future: impl Future<Output = ()> + Send + 'static) {
            async_std::task::spawn(future);
        }

        fn set_current_thread(thread: Option<Arc<dyn Any + Send + Sync>>) {
            CURRENT_THREAD.with(|t| *t.borrow_mut() = thread);
        }

        fn get_current_thread() -> Option<Arc<dyn Any + Send + Sync>> {
            CURRENT_THREAD.try_with(|t| {
                t.borrow().as_ref().cloned()
            }).unwrap_or(None)
        }
    }
}
