// use alloc::collections::linked_list::LinkedList;
// use core::cell::UnsafeCell;
// use core::future::Future;
// use core::ops::{Deref, DerefMut};
// use core::pin::Pin;
// use core::sync::atomic::{AtomicBool, Ordering};
// use core::task::{Context, Poll, Waker};

// use crate::spinlock::SpinLock;

// /// A mutual exclusion and asynchronous primitive which could work
// /// in bare metal environments.
// ///
// /// This mutex block coroutine waiting for the lock to become available.
// /// The mutex can also be statically initialized or created via a new
// /// constructor.  Each mutex has a type parameter which represents the
// /// data that it is protecting. The data can only be accessed through
// /// the RAII guards returned from lock and try_lock, which guarantees
// /// that the data is only ever accessed when the mutex is locked.
// pub struct AMutex<T: ?Sized> {
//     state: AtomicBool,
//     wakers: SpinLock<LinkedList<Waker>>,
//     data: UnsafeCell<T>,
// }

// /// An RAII implementation of a "scoped lock" of a mutex. When this structure is
// /// dropped (falls out of scope), the lock will be unlocked.
// ///
// /// The data protected by the mutex can be accessed through this guard via its
// /// [`Deref`] and [`DerefMut`] implementations.
// ///
// /// This structure is created by the [`lock`] and [`try_lock`] methods on
// /// [`AMutex`].
// ///
// /// [`lock`]: AMutex::lock
// /// [`try_lock`]: AMutex::try_lock
// #[must_use = "if unused the AMutex will immediately unlock"]
// pub struct AMutexGuard<'a, T: ?Sized> {
//     mutex: &'a AMutex<T>,
// }

// /// A future which resolves when the target mutex has been successfully
// /// acquired.
// pub struct AMutexLockFuture<'a, T: ?Sized> {
//     mutex: &'a AMutex<T>,
// }

// unsafe impl<T: ?Sized + Send> Send for AMutex<T> {}
// unsafe impl<T: ?Sized + Send> Sync for AMutex<T> {}
// unsafe impl<T: ?Sized + Send> Send for AMutexGuard<'_, T> {}
// unsafe impl<T: ?Sized + Sync> Sync for AMutexGuard<'_, T> {}
// unsafe impl<T: ?Sized + Send> Send for AMutexLockFuture<'_, T> {}

// impl<T> AMutex<T> {
//     /// Creates a new mutex in an unlocked state ready for use.
//     pub fn new(t: T) -> Self {
//         AMutex {
//             state: AtomicBool::new(false),
//             wakers: SpinLock::new(LinkedList::new()),
//             data: UnsafeCell::new(t),
//         }
//     }
// }

// impl<T: ?Sized> AMutex<T> {
//     pub fn lock(&self) -> AMutexLockFuture<'_, T> {
//         return AMutexLockFuture { mutex: self };
//     }

//     /// Attempts to acquire this lock immedidately.
//     pub fn try_lock(&self) -> Option<AMutexGuard<'_, T>> {
//         if !self.state.fetch_or(true, Ordering::Acquire) {
//             Some(AMutexGuard { mutex: self })
//         } else {
//             None
//         }
//     }

//     pub fn unlock(&self) {
//         self.state.store(false, Ordering::Release);
//         let waker = self.wakers.lock().pop_front();
//         if waker.is_some() {
//             waker.unwrap().wake();
//         }
//     }

//     pub fn register(&self, waker: Waker) {
//         self.wakers.lock().push_back(waker);
//     }
// }

// impl<'a, T: ?Sized> Future for AMutexLockFuture<'a, T> {
//     type Output = AMutexGuard<'a, T>;

//     fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
//         if let Some(lock) = self.mutex.try_lock() {
//             return Poll::Ready(lock);
//         }
//         let waker = cx.waker().clone();
//         self.mutex.register(waker);
//         if let Some(lock) = self.mutex.try_lock() {
//             return Poll::Ready(lock);
//         }
//         Poll::Pending
//     }
// }

// impl<T: ?Sized> Deref for AMutexGuard<'_, T> {
//     type Target = T;

//     #[inline]
//     fn deref(&self) -> &T {
//         unsafe { &*self.mutex.data.get() }
//     }
// }

// impl<T: ?Sized> DerefMut for AMutexGuard<'_, T> {
//     #[inline]
//     fn deref_mut(&mut self) -> &mut T {
//         unsafe { &mut *self.mutex.data.get() }
//     }
// }

// impl<T: ?Sized> Drop for AMutexGuard<'_, T> {
//     #[inline]
//     fn drop(&mut self) {
//         self.mutex.unlock();
//     }
// }
