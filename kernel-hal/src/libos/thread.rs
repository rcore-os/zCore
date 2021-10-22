use async_std::task_local;
use core::{cell::Cell, future::Future, pin::Pin};

task_local! {
    static TID: Cell<u64> = Cell::new(0);
    static PID: Cell<u64> = Cell::new(0);
}

hal_fn_impl! {
    impl mod crate::hal_fn::thread {
        fn spawn(future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>, _vmtoken: usize) {
            async_std::task::spawn(future);
        }

        fn set_tid(tid: u64, pid: u64) {
            TID.with(|x| x.set(tid));
            PID.with(|x| x.set(pid));
        }

        fn get_tid() -> (u64, u64) {
            let tid = TID.try_with(|x| x.get()).unwrap_or(0);
            let pid = PID.try_with(|x| x.get()).unwrap_or(0);
            (tid, pid)
        }
    }
}
