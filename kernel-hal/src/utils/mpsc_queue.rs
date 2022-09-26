use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

pub struct MpscQueue<'a, T: Copy> {
    pub size: usize,
    pub chead: AtomicUsize,
    pub phead: AtomicUsize,
    pub ptail: AtomicUsize,
    /// Safety:
    ///
    /// Access conflicts are avoided via atomic variables
    queue: UnsafeCell<&'a mut [T]>,
}

#[allow(unsafe_code)]
unsafe impl<'a, T: Copy> Sync for MpscQueue<'a, T> {}
#[allow(unsafe_code)]
unsafe impl<'a, T: Copy> Send for MpscQueue<'a, T> {}

impl<'a, T: Copy> MpscQueue<'a, T> {
    pub fn new(queue: &'a mut [T]) -> Self {
        Self {
            size: queue.len(),
            chead: AtomicUsize::new(0),
            phead: AtomicUsize::new(0),
            ptail: AtomicUsize::new(0),
            queue: UnsafeCell::new(queue),
        }
    }

    #[allow(clippy::mut_from_ref)]
    #[allow(unsafe_code)]
    pub fn entry_at(&self, idx: usize) -> &mut T {
        let queue = unsafe { &mut *self.queue.get() };
        &mut queue[idx % self.size]
    }

    pub fn chead(&self) -> usize {
        self.chead.load(Ordering::Acquire)
    }

    pub fn phead(&self) -> usize {
        self.phead.load(Ordering::Acquire)
    }

    pub fn ptail(&self) -> usize {
        self.ptail.load(Ordering::Acquire)
    }

    pub fn alloc_entry(&self) -> Option<usize> {
        loop {
            let chead = self.chead();
            let phead = self.phead();
            if phead - chead < self.size {
                if self
                    .phead
                    .compare_exchange(phead, phead + 1, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    break Some(phead);
                }
            } else {
                // notify consumer ?
                break None;
            }
        }
    }

    pub fn commit_entry(&self, idx: usize) -> bool {
        const RETRY_TIMES: usize = 100;
        let mut count = 0;
        while self.ptail() != idx {
            count += 1;
            if count > RETRY_TIMES {
                return false;
            }
        }
        self.ptail.fetch_add(1, Ordering::SeqCst);
        true
    }

    pub fn consume_entrys(&self) -> Vec<(usize, T)> {
        let mut vec = Vec::new();
        let chead = self.chead();
        let ptail = self.ptail();
        for idx in chead..ptail {
            vec.push((idx, *self.entry_at(idx)));
        }
        self.chead.store(ptail, Ordering::Release);
        vec
    }
}
