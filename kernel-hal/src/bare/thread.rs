//! Thread spawning.

use core::future::Future;

hal_fn_impl! {
    impl mod crate::hal_fn::thread {
        fn spawn(future: impl Future<Output = ()> + Send + 'static) {
            executor::spawn(future);
        }

        fn set_tid(_tid: u64, _pid: u64) {}

        fn get_tid() -> (u64, u64) {
            (0, 0)
        }
    }
}
