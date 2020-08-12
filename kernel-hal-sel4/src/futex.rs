use alloc::collections::btree_map::BTreeMap;
use alloc::collections::linked_list::LinkedList;
use crate::types::*;
use crate::error::*;
use crate::kipc::{kipc_loop, KipcLoopOutput, KipcChannel, SavedReplyHandle};
use crate::kt;
use lazy_static::lazy_static;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

lazy_static! {
    static ref FC: KipcChannel<FutexRequest> = KipcChannel::new().expect("futex/FC: init failed");
}

static FC_INIT: AtomicBool = AtomicBool::new(false);

enum FutexRequest {
    Wait(*const AtomicU32, u32),
    Wake(*const AtomicU32, usize),
}

unsafe impl Send for FutexRequest {}

struct FutexTracker {
    waiters: BTreeMap<usize, LinkedList<SavedReplyHandle>>,
}

impl FutexTracker {
    fn new() -> Self {
        FutexTracker {
            waiters: BTreeMap::new(),
        }
    }
}

pub fn init() {
    kt::spawn(|| {
        kt_futexd();
    }).expect("cannot spawn futexd");
    FC_INIT.store(true, Ordering::Relaxed);
}

fn ensure_fc_init() {
    if !FC_INIT.load(Ordering::Relaxed) {
        panic!("ensure_fc_init: Futex not yet initialized");
    }
}

fn kt_futexd() -> ! {
    let mut tracker = FutexTracker::new();

    kipc_loop(&*FC, |msg, reply| {
        match msg {
            FutexRequest::Wait(addr, old_value) => {
                let value = unsafe { (*addr).load(Ordering::Relaxed) };
                if value == old_value {
                    let handle = reply.save().expect("futexd: cannot save reply object");
                    tracker.waiters.entry(addr as usize).or_insert(LinkedList::new()).push_back(handle);
                    KipcLoopOutput::NoReply
                } else {
                    KipcLoopOutput::Reply(reply, 0)
                }
            }
            FutexRequest::Wake(addr, num_waiters) => {
                if let Some(waiters) = tracker.waiters.get_mut(&(addr as usize)) {
                    for _ in 0..num_waiters {
                        if let Some(handle) = waiters.pop_front() {
                            handle.send(0);
                        } else {
                            break;
                        }
                    }
                    if waiters.len() == 0 {
                        tracker.waiters.remove(&(addr as usize));
                    }
                }
                KipcLoopOutput::Reply(reply, 0)
            }
        }
    })
}

#[derive(Default)]
pub struct FSem {
    waiters: AtomicU32,
    count: AtomicU32,
}

impl FSem {
    pub const fn new(initial_count: u32) -> FSem {
        Self {
            waiters: AtomicU32::new(0),
            count: AtomicU32::new(initial_count),
        }
    }

    pub fn up(&self) {
        let new_val = self.count.fetch_add(1, Ordering::Release) + 1;
        if self.waiters.load(Ordering::Relaxed) > 0 {
            ensure_fc_init();
            FC.call(FutexRequest::Wake(&self.count, 1)).expect("FutexRequest::Wake failed");
        }
    }

    pub fn down(&self) {
        loop {
            let current_val = self.count.load(Ordering::Relaxed);
            if current_val > 0 && self.count.compare_and_swap(current_val, current_val - 1, Ordering::Acquire) == current_val {
                break;
            }
            self.waiters.fetch_add(1, Ordering::Acquire);
            ensure_fc_init();
            FC.call(FutexRequest::Wait(&self.count, current_val)).expect("FutexRerquest::Wait failed");
            self.waiters.fetch_sub(1, Ordering::Release);
        }
    }
}

pub struct FMutex<T> {
    inner: UnsafeCell<T>,
    sem: FSem,
}

unsafe impl<T: Send> Send for FMutex<T> {}
unsafe impl<T: Send> Sync for FMutex<T> {}

impl<T> FMutex<T> {
    pub const fn new(inner: T) -> FMutex<T> {
        FMutex {
            inner: UnsafeCell::new(inner),
            sem: FSem::new(1),
        }
    }

    pub fn lock<'a>(&'a self) -> FMutexGuard<'a, T> {
        self.sem.down();
        FMutexGuard { parent: self }
    }

    fn unlock(&self) {
        self.sem.up();
    }
}

pub struct FMutexGuard<'a, T> {
    parent: &'a FMutex<T>,
}

impl<'a, T> Deref for FMutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.parent.inner.get() }
    }
}

impl<'a, T> DerefMut for FMutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.parent.inner.get() }
    }
}

impl<'a, T> Drop for FMutexGuard<'a, T> {
    fn drop(&mut self) {
        self.parent.unlock();
    }
}

pub fn debug_wake_null() {
    FC.call(FutexRequest::Wake(core::ptr::null(), 1)).expect("debug_wake_null: FutexRequest::Wake failed");
}
