use crate::waker_page::{WakerPage, WakerRef, WAKER_PAGE_SIZE};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use bit_iter::BitIter;
use core::ops::{Coroutine, CoroutineState};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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
    /// Optional CPU affinity mask: bit `i` set means the task may run on the
    /// logical CPU `i`. `None` means "run anywhere" (no restriction, no extra
    /// allocation). The mask lives behind an `Arc<AtomicU64>` so a thread can
    /// change its own affinity at runtime (`sched_setaffinity`) and the
    /// scheduler observes the new value on the next placement/steal decision.
    affinity: Option<Arc<AtomicU64>>,
    /// The task's one shared waker, built at insertion. Every poll previously
    /// constructed two fresh `WakerRef`s (four `Arc` refcount RMWs) plus an
    /// `Arc::new` heap allocation; reusing a single allocation removes all of
    /// that from the hot poll path, and makes `Waker::will_wake` comparisons
    /// meaningful (same data pointer across polls), which keeps waker
    /// registries (net RX, sleep slots) deduplicated.
    waker: spin::Once<Arc<WakerRef>>,
}

struct TaskInner {
    priority: usize,
    state: TaskState,
    intr_enable: bool,
}

impl core::fmt::Debug for Task {
    fn fmt(&self, f: &mut Formatter) -> Result {
        let inner = crate::diag::diag_lock(&self.inner);
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
    pub fn new(
        future: impl Future<Output = ()> + Send + 'static,
        priority: usize,
        affinity: Option<Arc<AtomicU64>>,
    ) -> Self {
        Self {
            id: alloc_id(),
            future: Mutex::new(Box::pin(future)),
            inner: Mutex::new(TaskInner {
                priority,
                state: TaskState::RUNNABLE,
                intr_enable: false,
            }),
            finish: Arc::new(AtomicBool::new(false)),
            affinity,
            waker: spin::Once::new(),
        }
    }

    /// The task's shared waker. Set exactly once by `FutureCollection::insert`
    /// right after the slab slot and waker page exist; every later access is a
    /// plain acquire load.
    pub fn waker(&self) -> &Arc<WakerRef> {
        self.waker.get().expect("task waker not initialized")
    }

    /// Whether this task is allowed to be polled on the given logical CPU.
    ///
    /// A task with no affinity mask runs anywhere. CPU ids `>= 64` are always
    /// allowed (the mask only tracks the first 64 logical CPUs, which matches
    /// `MAX_CORE_NUM`).
    pub fn allowed_on(&self, cpu: usize) -> bool {
        match &self.affinity {
            None => true,
            Some(mask) => cpu >= 64 || (mask.load(Ordering::Relaxed) >> cpu) & 1 != 0,
        }
    }

    /// The task's affinity mask, or `None` when it may run anywhere.
    pub fn affinity_mask(&self) -> Option<u64> {
        self.affinity.as_ref().map(|m| m.load(Ordering::Relaxed))
    }
    pub fn poll(&self, cx: &mut Context) -> Poll<()> {
        // Never poll a task whose future already completed. `finish` is set by
        // `drop_by_ref` the instant a poll returns Ready, BEFORE the generator
        // removes the slab slot and frees the future Box. A stale waker (a
        // net-RX / IoMultiplexWait timer still holding a clone) that re-notifies
        // in that window could otherwise get the slot handed out and re-polled,
        // running the completed future again — its captured state
        // (interest_list buffer, process path) is being torn down, so the
        // re-poll dereferenced freed+poisoned heap (the intermittent
        // sys_epoll_pwait #GP with 0xa5.. in the faulting register). `self` is
        // the Arc<Task> the executor holds, so reading `finish` here is always
        // safe.
        if self.finish.load(Ordering::Relaxed) {
            return Poll::Ready(());
        }
        let mut f = crate::diag::diag_lock(&self.future);
        // (Deliberately no vtable-sniffing "corruption check" here: reading a
        // trait object's {data, vtable} fields via transmute_copy assumes a
        // layout Rust doesn't guarantee, and on master this exact pattern
        // (bbddbb56) caused false positives that silently completed healthy
        // tasks -- 6,500-30,000 busy-polls/s and a deterministic labwc crash
        // at ~35s, fixed by removing it entirely (PR #759). Don't reintroduce
        // it via a future merge from master.)
        f.as_mut().poll(cx)
    }

    /// Retire a future that was interrupted **in the middle of a poll** and
    /// must never be entered again.
    ///
    /// Used by the kernel's panic-containment path (`zcore::oops`): a poll that
    /// panicked left its coroutine stack half-unwound, so the generator's saved
    /// state no longer describes the values it holds — resuming it could
    /// re-run the faulting code, and *dropping* it could double-drop whatever
    /// the aborted poll had already moved out. Neither is acceptable, so the
    /// future is swapped out for an inert `Pending` and the original is
    /// deliberately leaked. One dead future's worth of memory, per contained
    /// fault, is the price of not halting the machine.
    ///
    /// Setting `finish` first means a late waker that gets this task handed out
    /// again finds `poll` returning `Ready` immediately, so the swapped-in
    /// `Pending` is never actually polled either.
    ///
    /// Returns `false` if the future lock could not be reclaimed, which leaves
    /// the task exactly as it was — the caller must then fall back to halting.
    ///
    /// # Safety
    ///
    /// Only the CPU that was polling this task may call this, and only while
    /// that poll is being abandoned: it force-releases the future lock that the
    /// abandoned poll still holds.
    pub unsafe fn abandon(&self) -> bool {
        // The panicking poll holds `future`'s lock and will never release it.
        self.future.force_unlock();
        let Some(mut slot) = self.future.try_lock() else {
            // Someone else took it between the unlock and here — do not touch
            // a future another CPU may be polling.
            return false;
        };
        self.finish.store(true, Ordering::SeqCst);
        let dead = core::mem::replace(
            &mut *slot,
            Box::pin(core::future::pending::<()>()) as Pin<Box<dyn Future<Output = ()> + Send>>,
        );
        core::mem::forget(dead);
        true
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
    /// Logical CPU that owns this collection; stamped into every `WakerPage`
    /// so a cross-CPU wake knows which CPU to kick with a reschedule IPI.
    pub cpu_id: u8,
}

impl FutureCollection {
    pub fn new(priority: usize, cpu_id: u8) -> Self {
        Self {
            slab: PinSlab::new(),
            // vec: VecDeque::new(),
            pages: vec![],
            priority,
            cpu_id,
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
    pub fn insert<F: Future<Output = ()> + 'static + Send>(
        &mut self,
        future: F,
        affinity: Option<Arc<AtomicU64>>,
    ) -> Key {
        let key = self
            .slab
            .insert(Arc::new(Task::new(future, self.priority, affinity)));
        // Add a new page to hold this future's status if the current page is filled.
        while key >= self.pages.len() * WAKER_PAGE_SIZE {
            self.pages.push(WakerPage::new(self.cpu_id));
        }
        let (page, subpage_idx) = self.page(key);
        page.initialize(subpage_idx);
        // Build the task's one shared waker now that its page slot exists.
        // (Clone the page Arc first so the `pages` borrow ends before the
        // mutable `slab` access below.)
        let page = page.clone();
        let task = self.slab.get(key).unwrap();
        let waker = Arc::new(page.make_waker(subpage_idx, &task.finish));
        task.waker.call_once(|| waker);
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
                .push(Mutex::new(FutureCollection::new(priority, cpu_id)));
        }
        tc.generator = Some(Mutex::new(Box::pin(TaskCollection::generator(tc_clone))));
        task_collection
    }

    /// 插入一个Future, 其优先级为 DEFAULT_PRIORITY
    pub fn add_task<F: Future<Output = ()> + 'static + Send>(
        &self,
        future: F,
        affinity: Option<Arc<AtomicU64>>,
    ) -> usize {
        self.priority_add_task(DEFAULT_PRIORITY, future, affinity)
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
        affinity: Option<Arc<AtomicU64>>,
    ) -> Key {
        debug_assert!(priority == DEFAULT_PRIORITY);
        let key =
            crate::diag::diag_lock(&self.future_collections[priority]).insert(future, affinity);
        debug_assert!(key < TASK_NUM_PER_PRIORITY);
        self.task_num.fetch_add(1, Ordering::Relaxed);
        key | (priority << PRIORITY_SHIFT)
    }

    fn get_mut_inner(&self, priority: usize) -> MutexGuard<'_, FutureCollection> {
        crate::diag::diag_lock(&self.future_collections[priority])
    }

    pub fn task_num(&self) -> usize {
        self.task_num.load(Ordering::Relaxed)
    }

    /// Diagnostics: `(task_num, notified_bits, dropped_bits, borrowed_bits)`
    /// summed across all waker pages. Used by the executor's hang detector to
    /// tell a lost wake (tasks present, notified == 0) from a take_task bug
    /// (notified > 0 yet nothing is polled).
    pub fn debug_pending(&self) -> (usize, u32, u32, u32) {
        let (mut n, mut d, mut b) = (0u32, 0u32, 0u32);
        for fc in &self.future_collections {
            let inner = crate::diag::diag_lock(fc);
            for page in &inner.pages {
                let (pn, pd, pb) = page.peek();
                n += pn.count_ones();
                d += pd.count_ones();
                b += pb.count_ones();
            }
        }
        (self.task_num(), n, d, b)
    }

    /// Non-blocking take for the work-stealing path: if the generator is busy
    /// (the owning CPU is inside its own `take_task`, possibly parked there by
    /// a timer preemption), give up instead of spinning. A thief that spins
    /// here while holding the victim's runtime lock deadlocks against the
    /// victim's timer IRQ (see `steal_task_from_other_cpu`).
    pub fn try_take_task(&self) -> Option<(Key, Arc<Task>, Arc<WakerRef>)> {
        let mut generator = self.generator.as_ref().unwrap().try_lock()?;
        self.resume_generator(&mut generator)
    }

    pub fn take_task(&self) -> Option<(Key, Arc<Task>, Arc<WakerRef>)> {
        let mut generator = crate::diag::diag_lock(self.generator.as_ref().unwrap());
        self.resume_generator(&mut generator)
    }

    /// Whether any task on this queue has a published (pending) wake. Pre-halt
    /// recheck for the executor: `try_lock` so a peer mid-insert makes us
    /// conservatively report "ready" instead of spinning — the caller simply
    /// skips the halt and re-runs `take_task`.
    pub fn has_ready(&self) -> bool {
        let cpu = crate::arch::cpu_id() as usize;
        self.future_collections.iter().any(|fc| {
            match fc.try_lock() {
                Some(mut inner) => {
                    for page_idx in 0..inner.pages.len() {
                        let page = &inner.pages[page_idx];
                        let (notified, dropped, borrowed) = page.peek();
                        let runnable = notified & !dropped & !borrowed;
                        if runnable != 0 {
                            for subpage_idx in BitIter::from(runnable) {
                                let key = pack_key(DEFAULT_PRIORITY, page_idx, subpage_idx);
                                let allowed = inner
                                    .slab
                                    .get(unmask_priority(key))
                                    .map(|task| task.allowed_on(cpu))
                                    .unwrap_or(true);
                                if allowed {
                                    return true;
                                }
                            }
                        }
                    }
                    false
                }
                // Keep this `true`: a peer mid-insert/mid-drain holds the lock, and
                // reporting `false` here would let a CPU halt through a wake it could
                // not yet observe, with no timer backstop in the executor's idle path.
                // (master's 1b1d289b/bbddbb56 shipped this as `false` and reintroduced
                // exactly that lost-wake class of bug; reverted there by PR #759 -- do
                // not let a future merge from master bring it back here either.)
                None => true,
            }
        })
    }

    /// Number of tasks on this queue that are *runnable right now* (a wake is
    /// published and no executor holds them).
    ///
    /// This is the load figure the scheduler actually wants. [`task_num`] counts
    /// every task the collection owns, including the ones parked on a `Pending`
    /// future — a CPU hosting fifty sleeping daemons looked fifty times more
    /// loaded than a CPU spinning two CPU-bound threads, so placement pushed new
    /// work *onto* the busy CPU and work stealing probed the idle one first.
    ///
    /// `try_lock`: this runs on the placement and idle/steal paths, which must
    /// never spin on a peer's collection. A momentarily locked collection is
    /// reported as `None` and simply skipped by the caller.
    ///
    /// [`task_num`]: Self::task_num
    pub fn ready_num(&self) -> Option<usize> {
        let inner = self.future_collections[DEFAULT_PRIORITY].try_lock()?;
        Some(
            inner
                .pages
                .iter()
                .map(|p| {
                    let (notified, dropped, borrowed) = p.peek();
                    (notified & !dropped & !borrowed).count_ones() as usize
                })
                .sum(),
        )
    }

    /// Load figure for spawn PLACEMENT — includes the task being polled right
    /// now (`borrowed`), unlike [`ready_num`].
    ///
    /// `ready_num` deliberately excludes `borrowed` because a borrowed task is
    /// checked out to an executor and cannot be stolen; for choosing a steal
    /// target that is correct. For placement it is exactly wrong: a CPU pegged
    /// running a CPU-bound hog has that hog `borrowed`, so `ready_num` reports
    /// it as 0 — indistinguishable from a truly idle CPU. Under 2N hogs on N
    /// CPUs the fork-storm placement scan then stacks new hogs onto whichever
    /// CPUs happen to be mid-poll (load 0) and never rebalances, so one CPU ends
    /// up with 4-5 hogs at 1/5 share each while another runs one at full speed —
    /// the measured 4.46x max/min unfairness. Counting `borrowed` as load makes
    /// a busy CPU advertise load >= 1, so hogs spread ~evenly. Reads the same
    /// page bits, adds no lock, and touches only the (cold) placement path.
    pub fn placement_load(&self) -> Option<usize> {
        let inner = self.future_collections[DEFAULT_PRIORITY].try_lock()?;
        Some(
            inner
                .pages
                .iter()
                .map(|p| {
                    let (notified, dropped, borrowed) = p.peek();
                    ((notified | borrowed) & !dropped).count_ones() as usize
                })
                .sum(),
        )
    }

    #[allow(clippy::type_complexity)]
    fn resume_generator(
        &self,
        generator: &mut Pin<Box<dyn Coroutine<Yield = Option<Key>, Return = ()>>>,
    ) -> Option<(Key, Arc<Task>, Arc<WakerRef>)> {
        match generator.as_mut().resume(()) {
            CoroutineState::Yielded(key) => {
                if let Some(key) = key {
                    let (priority, _page_idx, _subpage_idx) = unpack_key(key);
                    let mut inner = self.get_mut_inner(priority);
                    let task = inner.slab.get(unmask_priority(key)).unwrap().clone();
                    // The task's shared waker doubles as the borrow/drop handle,
                    // so the hot path no longer builds fresh `WakerRef`s (and an
                    // `Arc::new`) on every single poll.
                    let waker = task.waker().clone();
                    Some((key, task, waker))
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
                            let cpu = crate::arch::cpu_id() as usize;
                            for subpage_idx in BitIter::from(notified) {
                                // the key corresponding to the task
                                let key = pack_key(priority, page_idx, subpage_idx);
                                // Honor CPU affinity: a task may only be polled
                                // on a CPU allowed by its mask. If the CPU that
                                // is currently draining this collection (its own
                                // executor, or a thief during work stealing) is
                                // not allowed, re-arm the notified bit and skip
                                // it — an allowed CPU will pick it up on its next
                                // scan or steal. Leaving the bit set keeps the
                                // task discoverable instead of losing the wakeup.
                                let allowed = inner
                                    .slab
                                    .get(unmask_priority(key))
                                    .map(|task| task.allowed_on(cpu))
                                    .unwrap_or(true);
                                if !allowed {
                                    inner.pages[page_idx].notify(subpage_idx);
                                    // Forward the wake to a CPU that MAY run
                                    // it. Re-arming the bit alone left the task
                                    // discoverable but told nobody: a pinned
                                    // task woken from another CPU sat notified
                                    // in this collection until an allowed CPU's
                                    // next tick woke it to steal — measured as
                                    // a 7.4 ms (two-tick) cross-CPU pipe round
                                    // trip against 60 us same-CPU. One kick
                                    // through `request_resched`'s existing
                                    // coalescing turns that into an IPI.
                                    let mask = inner
                                        .slab
                                        .get(unmask_priority(key))
                                        .and_then(|task| task.affinity_mask())
                                        .unwrap_or(u64::MAX);
                                    crate::runtime::kick_for_affinity(mask, cpu);
                                    continue;
                                }
                                found_key = Some(key);
                                // Mark the task borrowed ATOMICALLY here, under the
                                // inner lock and before releasing it to be polled.
                                // `take_notified` cleared this task's notified bit,
                                // but the executor only set the borrowed bit AFTER
                                // take_task returned — leaving a window where a wake
                                // (e.g. a network IRQ) re-notified the task and a
                                // second CPU's take_task/steal picked up the SAME
                                // task, polling one future (and its single
                                // UserContext + coroutine stack) on two CPUs at once
                                // -> corrupted context -> iret to junk -> #UD/#SS.
                                // Setting borrowed now makes any racing wake defer
                                // the task (take_notified re-publishes borrowed bits)
                                // until this poll releases the borrow.
                                inner.pages[page_idx].mark_borrowed(subpage_idx, true);
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
