use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;
use core::task::Waker;
use core::task::{Context, Poll};
use spin::Mutex;

/// Creates a new oneshot channel, returning the sender/receiver halves.
pub fn create<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Mutex::new(Inner {
        waker: None,
        value: None,
    }));
    let sender = Sender {
        inner: inner.clone(),
    };
    let receiver = Receiver { inner };
    (sender, receiver)
}

/// The receiving half of oneshot.
///
/// Messages sent to the channel can be retrieved using `.await`.
pub struct Receiver<T> {
    inner: Arc<Mutex<Inner<T>>>,
}

/// The sending-half of oneshot.
pub struct Sender<T> {
    inner: Arc<Mutex<Inner<T>>>,
}

struct Inner<T> {
    waker: Option<Waker>,
    value: Option<T>,
}

impl<T> Sender<T> {
    /// Send a value and consume the sender.
    pub fn push(self, value: T) {
        let mut inner = self.inner.lock();
        inner.value = Some(value);
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
    }
}

impl<T> Future for Receiver<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.inner.lock();
        if let Some(value) = inner.value.take() {
            return Poll::Ready(value);
        }
        if inner.waker.is_none() {
            inner.waker = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}
