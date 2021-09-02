use async_std::task_local;
use core::{cell::Cell, future::Future, pin::Pin};

task_local! {
    static TID: Cell<u64> = Cell::new(0);
    static PID: Cell<u64> = Cell::new(0);
}

pub fn spawn(future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>, _vmtoken: usize) {
    async_std::task::spawn(future);
}

pub fn set_tid(tid: u64, pid: u64) {
    TID.with(|x| x.set(tid));
    PID.with(|x| x.set(pid));
}

pub fn get_tid() -> (u64, u64) {
    (TID.with(|x| x.get()), PID.with(|x| x.get()))
}
