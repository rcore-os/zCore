use crate::waker_page::{DroperRef, WakerPage, WakerRef, WAKER_PAGE_SIZE};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use bit_iter::BitIter;
use core::ops::{Coroutine, CoroutineState};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::{Mutex, MutexGuard};
use unicycle::pin_slab::PinSlab;
use {
    alloc::boxed::Box,
    core::future::Future,
    core::pin::Pin,
    core::task::{Context, Poll},
};

use core::fmt::{Debug, Formatter, Result};

// #[allow(unused)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    // BLOCKED,
    RUNNABLE,
    RUNNING,
}

pub struct Task {
    id: usize,
    future: Mutex<Pin<Box<dyn Future<Output = ()> + Send>>>,
    inner: Mutex<TaskInner>,
    finish: Arc<AtomicBool>,
}

struct TaskInner {
    priority: usize,
    state: TaskState,
    intr_enable: bool,
}

impl core::fmt::Debug for Task {
    fn fmt(&self, f: &mut Formatter) -> Result {
        let inner = self.inner.lock();
        let mut f = f.debug_struct("X86PTE");
        f.field("priority", &inner.priority);
        f.field("state", &inner.state);
        f.field("intr_enable", &inner.intr_enable);
        f.finish()
    }
}

fn alloc_id() -> usize {
    static TASK_ID: AtomicUsize = AtomicUsize::new(1);
    TASK_ID.fetch_add(1, Ordering::SeqCst)
}

impl Task {
    pub fn new(future: impl Future<Output = ()> + Send + 'static, priority: usize) -> Self {
        Self {
            id: alloc_id(),
            future: Mutex::new(Box::pin(future)),
            inner: Mutex::new(TaskInner {
                priority,
                state: TaskState::RUNNABLE,
                intr_enable: false,
            }),
            finish: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn poll(&self, cx: &mut Context) -> Poll<()> {
        // if self.finish.load(Ordering::Relaxed) {
        //     return Poll::Ready(());
        // }
        let mut f = self.future.lock();
        if self.inner.lock().intr_enable {
            crate::arch::intr_on();
        }
        let ret = f.as_mut().poll(cx);
        self.inner.lock().intr_enable = crate::arch::intr_get();
        crate::arch::intr_off();
        ret
    }

    pub fn id(&self) -> usize {
        self.id
    }
}

pub struct FutureCollection {
    pub slab: PinSlab<Arc<Task>>,
    // pub vec: VecDeque<Key>,
    pub pages: Vec<Arc<WakerPage>>,
    pub priority: usize,
}

impl FutureCollection {
    pub fn new(priority: usize) -> Self {
        Self {
            slab: PinSlab::new(),
            // vec: VecDeque::new(),
            pages: vec![],
            priority,
        }
    }
    /// Our pages hold 64 contiguous future wakers, so we can do simple arithmetic to access the
    /// correct page as well as the index within page.
    /// Given the `key` representing a future, return a reference to that page, `Arc<WakerPage>`. And
    /// the index _within_ that page (usize).
    pub fn page(&self, key: Key) -> (&Arc<WakerPage>, usize) {
        let (_, page_idx, subpage_idx) = unpack_key(key);
        (&self.pages[page_idx], subpage_idx)
    }

    /// Insert a future into our scheduler returning an integer key representing this future. This
    /// key is used to index into the slab for accessing the future.
    pub fn insert<F: Future<Output = ()> + 'static + Send>(&mut self, future: F) -> Key {
        let key = self.slab.insert(Arc::new(Task::new(future, self.priority)));
        // Add a new page to hold this future's status if the current page is filled.
        while key >= self.pages.len() * WAKER_PAGE_SIZE {
            self.pages.push(WakerPage::new());
        }
        let (page, subpage_idx) = self.page(key);
        page.initialize(subpage_idx);
        // self.vec.push_back(key);
        key
    }

    pub fn remove(&mut self, key: Key) {
        let (page, subpage_idx) = self.page(key);
        page.clear(subpage_idx);
        self.slab.remove(unmask_priority(key));
    }
}

pub struct TaskCollection {
    cpu_id: u8, // Just for debug, not used
    future_collections: Vec<Mutex<FutureCollection>>,
    pub task_num: AtomicUsize,
    generator: Option<Mutex<Pin<Box<dyn Coroutine<Yield = Option<Key>, Return = ()>>>>>,
}

impl TaskCollection {
    pub fn new(cpu_id: u8) -> Arc<Self> {
        let mut task_collection = Arc::new(TaskCollection {
            cpu_id,
            future_collections: Vec::with_capacity(MAX_PRIORITY),
            task_num: AtomicUsize::new(0),
            generator: None,
        });
        // SAFETY: no other Arc or Weak pointers
        let tc_clone = task_collection.clone();
        let tc = unsafe { Arc::get_mut_unchecked(&mut task_collection) };
        for priority in 0..MAX_PRIORITY {
            tc.future_collections
                .push(Mutex::new(FutureCollection::new(priority)));
        }
        tc.generator = Some(Mutex::new(Box::pin(TaskCollection::generator(tc_clone))));
        task_collection
    }

    /// 插入一个Future, 其优先级为 DEFAULT_PRIORITY
    pub fn add_task<F: Future<Output = ()> + 'static + Send>(&self, future: F) -> usize {
        self.priority_add_task(DEFAULT_PRIORITY, future)
    }

    /// remove the task correponding to the key.
    pub fn remove_task(&self, key: Key) {
        let mut inner = self.get_mut_inner(key >> PRIORITY_SHIFT);
        inner.remove(unmask_priority(key));
        self.task_num.fetch_sub(1, Ordering::Relaxed);
    }

    fn priority_add_task<F: Future<Output = ()> + 'static + Send>(
        &self,
        priority: usize,
        future: F,
    ) -> Key {
        debug_assert!(priority == DEFAULT_PRIORITY);
        let key = self.future_collections[priority].lock().insert(future);
        debug_assert!(key < TASK_NUM_PER_PRIORITY);
        self.task_num.fetch_add(1, Ordering::Relaxed);
        key | (priority << PRIORITY_SHIFT)
    }

    fn get_mut_inner(&self, priority: usize) -> MutexGuard<'_, FutureCollection> {
        self.future_collections[priority].lock()
    }

    pub fn task_num(&self) -> usize {
        self.task_num.load(Ordering::Relaxed)
    }

    pub fn take_task(&self) -> Option<(Key, Arc<Task>, WakerRef, DroperRef)> {
        let mut generator = self.generator.as_ref().unwrap().lock();
        match generator.as_mut().resume(()) {
            CoroutineState::Yielded(key) => {
                if let Some(key) = key {
                    let (priority, page_idx, subpage_idx) = unpack_key(key);
                    let mut inner = self.get_mut_inner(priority);
                    let task = inner.slab.get(unmask_priority(key)).unwrap().clone();
                    let waker = inner.pages[page_idx].make_waker(subpage_idx, &task.finish);
                    let droper = waker.clone();
                    Some((key, task, waker, droper))
                } else {
                    None
                }
            }
            _ => panic!("unexpected value from resume"),
        }
    }

    pub fn generator(self: Arc<Self>) -> impl Coroutine<Yield = Option<Key>, Return = ()> {
        #[coroutine]
        static move || {
            loop {
                let priority = DEFAULT_PRIORITY;
                loop {
                    let mut found_key: Option<Key> = None;
                    let mut inner = self.get_mut_inner(priority);
                    for page_idx in 0..inner.pages.len() {
                        let page = &inner.pages[page_idx];
                        let notified = page.take_notified();
                        let dropped = page.take_dropped();
                        if notified != 0 {
                            for subpage_idx in BitIter::from(notified) {
                                // the key corresponding to the task
                                found_key = Some(pack_key(priority, page_idx, subpage_idx));
                                drop(inner);
                                yield found_key;
                                inner = self.get_mut_inner(priority);
                            }
                        }
                        if dropped != 0 {
                            for subpage_idx in BitIter::from(dropped) {
                                // the key corresponding to the task
                                let key = pack_key(priority, page_idx, subpage_idx);
                                self.task_num.fetch_sub(1, Ordering::Relaxed);
                                inner.remove(key);
                            }
                        }
                    }
                    if found_key.is_none() {
                        break;
                    }
                }
                yield None;
            }
        }
    }
}

pub use key::*;

pub mod key {
    pub type Key = usize;
    pub const PRIORITY_SHIFT: usize = 58;
    pub const TASK_NUM_PER_PRIORITY: usize = 1 << PRIORITY_SHIFT;
    pub const MAX_PRIORITY: usize = 1 << 5;
    pub const DEFAULT_PRIORITY: usize = 4;

    pub const PAGE_INDEX_SHIFT: usize = 6;

    pub fn unpack_key(key: Key) -> (usize, usize, usize) {
        let subpage_idx = key & 0x3F;
        let page_idx = (key << 5) >> 11;
        let priority = key >> PRIORITY_SHIFT;
        (priority, page_idx, subpage_idx)
    }

    pub fn pack_key(priority: usize, page_idx: usize, subpage_idx: usize) -> Key {
        (priority << PRIORITY_SHIFT) | (page_idx << PAGE_INDEX_SHIFT) | subpage_idx
    }

    pub fn unmask_priority(key: Key) -> usize {
        key & !(0x1F << PRIORITY_SHIFT)
    }
}
