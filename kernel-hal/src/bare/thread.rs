use alloc::boxed::Box;
use core::task::{Context, Poll};
use core::{future::Future, pin::Pin};

use spin::Mutex;

hal_fn_impl! {
    impl mod crate::hal_fn::thread {
        fn spawn(future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>, vmtoken: usize) {
            struct PageTableSwitchWrapper {
                inner: Mutex<Pin<Box<dyn Future<Output = ()> + Send>>>,
                vmtoken: usize,
            }
            impl Future for PageTableSwitchWrapper {
                type Output = ();
                fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                    crate::vm::activate_paging(self.vmtoken);
                    self.inner.lock().as_mut().poll(cx)
                }
            }

            executor::spawn(PageTableSwitchWrapper {
                inner: Mutex::new(future),
                vmtoken,
            });
        }

        fn set_tid(_tid: u64, _pid: u64) {}

        fn get_tid() -> (u64, u64) {
            (0, 0)
        }
    }
}
