use crate::context::{Context as ExecuterContext, ContextData};
use alloc::alloc::{Allocator, Global, Layout};
use core::pin::Pin;
use {
    alloc::boxed::Box,
    alloc::sync::Arc,
    core::ptr::NonNull,
    core::task::{Context, Poll},
};

use crate::arch::executor_entry;
use crate::task_collection::{Task, TaskCollection};

#[derive(Debug, PartialEq, Eq)]
enum ExecutorState {
    STRONG,
    WEAK, // 执行完一次future后就需要被drop
    KILLED,
    UNUSED,
}

pub struct Executor {
    id: usize,
    task_collection: Arc<TaskCollection>,
    stack_base: usize,
    pub context: ExecuterContext,
    #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
    context_data: ContextData,
    task_id: usize,
    state: ExecutorState,
}

/// Idle-loop iterations since any task was last polled (hang detector; see the
/// idle branch in `run`). Global because all executors on a CPU share progress.
static IDLE_STREAK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

// Scheduler-loop branch counters, surfaced via [`sched_stats`] for
// `/proc/perf/kernel` so a busy-spin can be attributed: `polled` = a task was
// available to run, `weak_yield` = no task but a weak executor outstanding so we
// spun via `sched_yield` instead of halting.
static SCHED_POLLED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static SCHED_WEAK_YIELD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// `(tasks polled, weak-executor yields)` since boot.
pub fn sched_stats() -> (u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    (SCHED_POLLED.load(Relaxed), SCHED_WEAK_YIELD.load(Relaxed))
}

const STACK_SIZE: usize = 4096 * 32;
const STACK_LAYOUT: Layout = Layout::new::<[u8; STACK_SIZE]>();

/// DEBUG: magic written to the lowest words of every coroutine stack. The stack
/// grows down from `stack_base + STACK_SIZE`; if a deep kernel call chain reaches
/// `stack_base`, the canary is clobbered and detected after the future yields.
const STACK_CANARY: u64 = 0x5354_4143_4b5f_4f56; // "STACK_OV"

fn executor_alloc_id() -> usize {
    use core::sync::atomic::{AtomicUsize, Ordering};
    static EXECUTOR_ID: AtomicUsize = AtomicUsize::new(1);
    EXECUTOR_ID.fetch_add(1, Ordering::SeqCst)
}

impl Executor {
    pub fn new(task_collection: Arc<TaskCollection>) -> Pin<Box<Self>> {
        let stack: NonNull<u8> = Global
            .allocate(STACK_LAYOUT)
            .expect("Alloction Stack Failed.")
            .cast();
        let stack_base = stack.as_ptr() as usize;
        // DEBUG: lay down the stack-overflow canary at the lowest 4 words.
        unsafe {
            let p = stack_base as *mut u64;
            for i in 0..4 {
                core::ptr::write_volatile(p.add(i), STACK_CANARY ^ i as u64);
            }
        }
        let mut pin_executor = Pin::new(Box::new(Executor {
            id: executor_alloc_id(),
            task_collection,
            stack_base,
            context: ExecuterContext::default(),
            #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
            context_data: ContextData::default(),
            task_id: 0,
            state: ExecutorState::UNUSED,
        }));

        pin_executor.init_stack_and_context();

        trace!(
            "stack top 0x{:x} executor addr 0x{:x}, pgbr = 0x{:x}",
            pin_executor.context.get_sp(),
            pin_executor.context.get_pc(),
            pin_executor.context.get_pgbr(),
        );
        pin_executor
    }

    // stack layout: [executor_addr | context ]
    fn init_stack_and_context(&mut self) {
        let mut stack_top = self.stack_base + STACK_SIZE;
        let self_addr = self as *const Self as usize;
        stack_top = unsafe { push_stack(stack_top, self_addr) };
        #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
        {
            self.context_data = ContextData::new(
                executor_entry as *const () as usize,
                stack_top,
                crate::arch::pg_base_register(),
            );
            self.context
                .set_context(&self.context_data as *const _ as usize);
        }
        #[cfg(target_arch = "x86_64")]
        {
            let context_data = ContextData::new(
                executor_entry as *const () as usize,
                stack_top,
                crate::arch::pg_base_register(),
            );
            stack_top = unsafe { push_stack(stack_top, context_data) };
            self.context.set_context(stack_top);
        }
    }

    pub fn run(&mut self) {
        // Lazy-TLB safety pin.
        //
        // `ThreadSwitchFuture::poll` leaves this CPU on the polled thread's
        // *process* page table after the poll (lazy-TLB: it skips reloading the
        // kernel CR3 to avoid a TLB flush per poll). The scheduler code below —
        // `take_task` and `steal_task_from_other_cpu` — therefore runs under
        // that user CR3. Those routines only touch kernel-half memory, which is
        // mapped in every process page table, so that is fine *as long as the
        // page table still exists*.
        //
        // The danger is the page table being freed out from under us: if the
        // process whose CR3 we are holding exits and is reaped on another CPU,
        // its `PageTableImpl` drops and the root (PML4) / intermediate frames go
        // back to the frame allocator and get reused. The MMU then walks freed,
        // overwritten page-table memory on the next TLB miss, so our own kernel
        // stack / the iret frame we are about to build reads garbage -> `iretq`
        // to ring-0 junk -> #UD. This is exactly the intermittent SMP crash seen
        // under `apk` (rapid fork/exit) — and ctxcheck never fires because the
        // saved `UserContext` is valid; it is the physical memory behind it that
        // changes under the stale CR3.
        //
        // Hold an `Arc` to the most-recently-polled task across the next
        // `take_task`/`steal` step. That keeps its `Thread` -> `Process` ->
        // `vmar` -> page table alive, so a concurrent exit cannot free the page
        // table while its CR3 is still loaded here. The pin is replaced only
        // after the *next* poll has switched CR3 to another address space (or to
        // the kernel CR3, which `CurrentThread::drop` restores when a thread
        // finishes), so the previous page table is released only once its CR3 is
        // no longer loaded on this CPU.
        let mut _cr3_pin: Option<Arc<Task>> = None;
        loop {
            let mut task_info = self.task_collection.take_task();
            if task_info.is_none() {
                task_info = crate::runtime::steal_task_from_other_cpu();
            }
            if let Some((_key, task, waker_ref)) = task_info {
                // `waker_ref` is the task's one shared waker (built at insert):
                // no per-poll `Arc::new`, and the borrowed bit was already set
                // atomically by the generator under the collection lock —
                // setting it again here was redundant.
                let waker = woke::waker_ref(&waker_ref);
                let mut cx = Context::from_waker(&waker);
                self.task_id = task.id();
                debug!("running future {}:{}", self.id(), task.id());
                // Hang detector: a task is being polled, so clear the idle-loop
                // streak. If the machine then spins the idle loop many times with
                // tasks still present but nothing polled, a wake was lost (see the
                // else-branch dump below).
                IDLE_STREAK.store(0, core::sync::atomic::Ordering::Relaxed);
                SCHED_POLLED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                let ret = task.poll(&mut cx);
                // DEBUG: did this future overflow the 128 KiB coroutine stack?
                unsafe {
                    let p = self.stack_base as *const u64;
                    for i in 0..4 {
                        if core::ptr::read_volatile(p.add(i)) != (STACK_CANARY ^ i as u64) {
                            error!(
                                "[stackcheck] COROUTINE STACK OVERFLOW: executor id={} task_id={} stack_base={:#x} size={:#x}",
                                self.id(), task.id(), self.stack_base, STACK_SIZE
                            );
                            break;
                        }
                    }
                }
                debug!("back from future {}:{}", self.id(), task.id());
                self.task_id = 0;
                // Pin this task's address space for the upcoming take_task/steal
                // (which run under the CR3 this poll just (re)loaded). Replacing
                // the previous pin here is safe: CR3 now points at *this* task's
                // page table (or at the kernel CR3 if the thread just finished —
                // `CurrentThread::drop` restored it), so the page table we drop
                // is no longer the active one. See the comment at the top of
                // `run`.
                _cr3_pin = Some(task.clone());
                // Borrow-release ordering — this is load-bearing for SMP.
                //
                // The OLD order (mark_borrowed(false), then drop_by_ref on
                // Ready) opened a window where a completed task was published
                // as (borrowed=0, dropped=0). A wake that raced with the poll
                // (deferred by take_notified while we were borrowed — routine
                // for IRQ-driven futures) then let ANOTHER executor take and
                // re-poll the SAME completed task. If a timer preemption
                // parked either executor in that window, the late
                // mark_borrowed(false) could land on a slab slot that had
                // been removed and REUSED, wiping the borrow bit of an
                // unrelated live task -> two executors polling one future ->
                // the second spins forever on the future lock
                // (task_collection.rs, Task::poll) while the first sits
                // parked as a weak executor: the >8s DEADLOCK banner.
                //
                // Therefore: on Ready, publish `dropped` FIRST and leave the
                // borrow bit SET. take_notified masks dropped tasks, so the
                // task can never be handed out again; the generator's
                // dropped-branch remove() -> clear() wipes all bits (borrow
                // included) atomically with freeing the slot, BEFORE the slot
                // can be reused by insert(). Only a Pending poll releases the
                // borrow here, and only after Task::poll has returned (future
                // lock already released).
                match ret {
                    Poll::Ready(()) => {
                        debug!("task over id = {}", task.id());
                        waker_ref.drop_by_ref();
                    }
                    Poll::Pending => {
                        waker_ref.mark_borrowed(false);
                    }
                };
                if let ExecutorState::WEAK = self.state {
                    self.state = ExecutorState::KILLED;
                    return;
                }
            } else {
                // Our run queue is drained (and stealing found nothing), so any
                // pending wake-up preemption request for this CPU has already
                // been satisfied by simply running out of work. Drop it: a stale
                // bit would suppress the coalesced IPI for the next real wake.
                crate::runtime::clear_need_resched(crate::arch::cpu_id() as usize);
                let runtime = crate::runtime::get_current_runtime();
                let task_num = runtime.task_num();
                let weak_executor = runtime.weak_executor_num();
                drop(runtime);
                // TODO: some cores may exit by mistake when we have multi-cores
                if cfg!(feature = "baremetal-test") && task_num == 0 {
                    debug!("all done! exit and reboot");
                    crate::runtime::sched_yield();
                } else if weak_executor != 0 {
                    debug!("return to runtime and run weak executor");
                    SCHED_WEAK_YIELD.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    crate::runtime::sched_yield();
                } else if crate::runtime::run_idle_callback() {
                    // The idle callback made progress (e.g. drained deferred
                    // driver jobs that may have woken tasks): re-check the run
                    // queue instead of halting until the next interrupt.
                    continue;
                } else {
                    // Hang detector (diagnostics only): the run queue is empty so
                    // we are about to halt until the next interrupt. If tasks still
                    // exist, count idle-loop iterations; the 250 Hz timer wakes us
                    // ~every 4 ms, so ~750 iterations with no task polled ≈ 3 s.
                    // `IDLE_STREAK` is global and reset on every real poll, so it
                    // only climbs while *no* CPU makes progress.
                    //
                    // Snapshot the page bits ONLY at report cadence:
                    // `debug_pending` walks every collection under its lock,
                    // far too heavy to run on each of the ~250 idle passes/s.
                    //
                    // Classification: `notified > 0` (affinity-parked) and
                    // `borrowed > 0` (in-flight on another executor) are normal
                    // under SMP; only tasks-exist-but-nothing-pending can never
                    // recover on its own = a genuine lost wake.
                    if task_num > 0 {
                        let s = IDLE_STREAK.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        if s == 750 || (s > 750 && s % 2500 == 0) {
                            let (tn, n, d, b) = self.task_collection.debug_pending();
                            if n == 0 && b == 0 {
                                warn!(
                                    "[sched] possible lost wake: {} task(s) parked \
                                     (notified=0 borrowed=0 dropped={}) after {} idle passes",
                                    tn, d, s
                                );
                            }
                        }
                    }
                    debug!("no other tasks, wait for interrupt");
                    // Halt protocol vs lost wakes. Publish "sleeping" FIRST,
                    // then re-check the queue with IRQs off, and only then
                    // halt (`wait_for_interrupt` is an atomic sti;hlt — an IPI
                    // arriving after the sti breaks the hlt). A remote waker
                    // does notify -> read sleeping-mask (both SeqCst): either
                    // it sees us sleeping and kicks us with the reschedule
                    // IPI, or its notify is ordered before our recheck and we
                    // see the ready bit and skip the halt. Either way the
                    // wake cannot fall into the check-then-halt window (which
                    // previously cost up to one full 4 ms tick).
                    let cpu = crate::arch::cpu_id() as usize;
                    let intr_was_on = crate::arch::intr_get();
                    crate::arch::intr_off();
                    crate::runtime::set_cpu_sleeping(cpu, true);
                    if !self.task_collection.has_ready() {
                        crate::arch::wait_for_interrupt();
                    }
                    crate::runtime::set_cpu_sleeping(cpu, false);
                    if intr_was_on {
                        crate::arch::intr_on();
                    }
                }
            }
        }
    }

    // 当前是否在运行future
    // 发生supervisor时钟中断时, 若executor在运行future, 则
    // 说明该future超时, 需要切换到另一个executor来执行其他future.
    pub fn is_running_future(&self) -> bool {
        self.task_id != 0
    }

    pub fn killed(&self) -> bool {
        self.state == ExecutorState::KILLED
    }

    pub fn mark_weak(&mut self) {
        self.state = ExecutorState::WEAK;
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn task_id(&self) -> usize {
        self.task_id
    }

    /// Base address of this executor's coroutine stack (lowest address).
    pub fn stack_base(&self) -> usize {
        self.stack_base
    }

    /// [diag] Whether the stack-overflow canary at the stack base is intact.
    ///
    /// The coroutine stack is a plain heap allocation with NO guard page: a
    /// runaway kernel call chain (e.g. unbounded recursion) silently writes
    /// frames straight through the base into neighbouring heap allocations —
    /// the corruption class behind the `/proc/self/exe` self-reference bug
    /// (see docs/README-crash-repro.md). Any overflow deep enough to matter
    /// passes THROUGH these 4 words, so checking them from the timer tick
    /// (`runtime::check_current_executor_canary`) converts silent heap
    /// corruption into a labelled panic within one tick (~4 ms).
    pub fn canary_intact(&self) -> bool {
        unsafe {
            let p = self.stack_base as *const u64;
            (0..4).all(|i| core::ptr::read_volatile(p.add(i)) == (STACK_CANARY ^ i as u64))
        }
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        unsafe {
            let stack = NonNull::<u8>::new_unchecked(self.stack_base as *mut u8);
            Global.deallocate(stack, STACK_LAYOUT);
        }
    }
}

unsafe impl Send for Executor {}
unsafe impl Sync for Executor {}

pub unsafe fn push_stack<T>(stack_top: usize, val: T) -> usize {
    let stack_top = (stack_top as *mut T).sub(1);
    *stack_top = val;
    stack_top as _
}
