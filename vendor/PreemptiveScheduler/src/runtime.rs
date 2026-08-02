use crate::{executor::Executor, task_collection::*, waker_page::WakerRef};

#[cfg(target_arch = "x86_64")]
use crate::context::Context;
#[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
use crate::context::ContextData as Context;

use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use core::{future::Future, pin::Pin};
use lazy_static::*;
use spin::{Mutex, MutexGuard};

/// Shared with the `lock` crate (and checked against `kernel_hal::config`) so
/// every per-CPU array in the system agrees on one size.
const MAX_CORE_NUM: usize = lock::MAX_CORE_NUM;

/// One past the highest dense logical cpu id that has entered `run_until_idle`.
/// Task placement and work stealing only consider CPUs in `0..num_online_cpus()`:
/// a task parked on the queue of a CPU that never runs an executor would only
/// ever execute if some idle CPU happened to steal it.
static NUM_ONLINE_CPUS: AtomicUsize = AtomicUsize::new(1);

#[inline]
pub(crate) fn num_online_cpus() -> usize {
    NUM_ONLINE_CPUS.load(Ordering::Relaxed)
}

/// Callback invoked by an executor when its CPU runs out of work, right before
/// halting to wait for an interrupt. Returns `true` if it made progress (e.g.
/// drained deferred driver jobs), in which case the executor re-checks its run
/// queue instead of halting.
static IDLE_CALLBACK: AtomicUsize = AtomicUsize::new(0);

pub fn set_idle_callback(f: fn() -> bool) {
    IDLE_CALLBACK.store(f as usize, Ordering::Release);
}

pub(crate) fn run_idle_callback() -> bool {
    let f = IDLE_CALLBACK.load(Ordering::Acquire);
    if f != 0 {
        let f: fn() -> bool = unsafe { core::mem::transmute(f) };
        f()
    } else {
        false
    }
}

// ─── Reschedule IPI: cross-CPU wake latency ────────────────────────────────
//
// Waking a task only sets a bit in the owning CPU's waker page. If that CPU is
// halted in `wait_for_interrupt`, nothing would make it look again until its
// next periodic tick — up to a full 4 ms of added latency on every cross-CPU
// wake (pipe writes, process exits, IO completions). The executor publishes
// "I am about to halt" in `SLEEPING_CPUS`; wakers and spawners check it and
// kick the target with an IPI registered by the HAL (the scheduler cannot
// call the HAL directly — that dependency is the other way around).

/// Bitmask of logical CPUs currently inside the halt window (bit i = CPU i).
static SLEEPING_CPUS: AtomicU64 = AtomicU64::new(0);

/// Bitmask of logical CPUs with a pending *wake-up preemption* request: a task
/// became runnable on that CPU while it was busy running a different task.
///
/// Without this, a freshly woken task had to wait for the running thread's full
/// timeslice (`BASE_TIMESLICE_TICKS` = 5 ticks = 20 ms at 250 Hz) before the
/// executor would even look at the run queue again — the executor polls a task
/// until it returns `Pending`, and a CPU-bound user thread only returns
/// `Pending` when its slice expires. Every interactive wake (a keystroke, a
/// pipe write, an I/O completion, a `futex` release) therefore paid up to 20 ms
/// of latency as soon as *anything* else was runnable on that CPU. That is the
/// single biggest reason the system feels slower than the micro-benchmarks
/// suggest: the benchmarks measure one task on an otherwise idle machine, where
/// this path never triggers.
///
/// Linux solves it with `check_preempt_curr` + a reschedule IPI. So do we: the
/// waker publishes the request here and kicks the target CPU, and the user-trap
/// path consumes it via [`take_need_resched`] and yields.
static NEED_RESCHED: AtomicU64 = AtomicU64::new(0);

/// Master switch for wake-up preemption, settable from the kernel command line
/// (`WAKEPREEMPT=0`).
///
/// Any scheduler change trades one workload against another, and the only
/// honest way to find out which is to run the same build both ways on the same
/// machine. Rebuilding between A and B does not qualify — the kernel binary
/// differs, so does its layout, and a QEMU/TCG run has enough variance to hide
/// the difference either way. This makes the comparison a boot parameter.
static WAKEUP_PREEMPT: AtomicBool = AtomicBool::new(true);

/// Enable/disable wake-up preemption. Called once at boot from the command
/// line; there is no reason to flip it at runtime.
pub fn set_wakeup_preempt(enabled: bool) {
    WAKEUP_PREEMPT.store(enabled, Ordering::Relaxed);
}

/// Whether wake-up preemption is enabled.
pub fn wakeup_preempt_enabled() -> bool {
    WAKEUP_PREEMPT.load(Ordering::Relaxed)
}

/// Wake-up preemption accounting, surfaced through [`wakeup_preempt_stats`]:
/// `(requests published, requests actually consumed by a yielding thread)`.
/// A large gap means requests are being raised on CPUs that are not in user
/// mode (so the trap path never sees them) — worth knowing before tuning.
static RESCHED_REQUESTED: AtomicU64 = AtomicU64::new(0);
static RESCHED_TAKEN: AtomicU64 = AtomicU64::new(0);

/// `(wake-up preemption requests, requests honoured)` since boot.
pub fn wakeup_preempt_stats() -> (u64, u64) {
    (
        RESCHED_REQUESTED.load(Ordering::Relaxed),
        RESCHED_TAKEN.load(Ordering::Relaxed),
    )
}

/// HAL-registered function that sends a wake IPI to a logical CPU.
static RESCHED_IPI_SENDER: AtomicUsize = AtomicUsize::new(0);

/// Register the IPI sender used to kick halted CPUs on cross-CPU wakes.
pub fn set_resched_ipi_sender(f: fn(usize)) {
    RESCHED_IPI_SENDER.store(f as usize, Ordering::Release);
}

/// Executor-side: publish/clear this CPU's "about to halt" flag. SeqCst so the
/// flag RMW orders against the subsequent queue recheck (see `Executor::run`).
pub(crate) fn set_cpu_sleeping(cpu: usize, sleeping: bool) {
    if cpu >= 64 {
        return;
    }
    if sleeping {
        SLEEPING_CPUS.fetch_or(1 << cpu, Ordering::SeqCst);
    } else {
        SLEEPING_CPUS.fetch_and(!(1 << cpu), Ordering::SeqCst);
    }
}

#[inline]
fn send_resched_ipi(owner: usize) {
    let f = RESCHED_IPI_SENDER.load(Ordering::Acquire);
    if f != 0 {
        let f: fn(usize) = unsafe { core::mem::transmute(f) };
        f(owner);
    }
}

/// Waker-side: kick `owner` with the wake IPI if it is (about to be) halted.
///
/// The plain delivery kick, with no preemption request attached: used for a
/// wake whose task is already checked out to an executor, where there is
/// nothing to preempt for but the wake still has to reach a halted owner.
/// The caller must have already published the wake (SeqCst) — see the pairing
/// argument in `WakerRef::wake_by_ref`.
#[inline]
pub(crate) fn maybe_send_resched_ipi(owner: u8) {
    let owner = owner as usize;
    if owner >= 64 || SLEEPING_CPUS.load(Ordering::SeqCst) & (1 << owner) == 0 {
        return;
    }
    send_resched_ipi(owner);
}

/// Waker-side: a task just became runnable on `owner`. Make that CPU look at
/// its run queue *soon* instead of at the end of the running thread's timeslice.
///
/// Two cases, both handled here:
///
///  * **`owner` is halted** — kick it with the wake IPI so it leaves `hlt`
///    (previous behaviour, preserved unconditionally: a CPU can enter the halt
///    window after an earlier request already set its bit).
///  * **`owner` is busy running another task** — publish a reschedule request
///    and, on the 0→1 transition only, send the IPI so the CPU takes a trap
///    promptly. `handle_user_trap` then sees [`take_need_resched`] and yields,
///    handing the CPU to the woken task. Coalescing on the transition keeps a
///    burst of wakes (a 64-entry socket queue draining, N threads released from
///    one futex) at one IPI instead of N.
///
/// Sending to *ourselves* is pointless — we are already executing on this CPU
/// and will reach the trap path without an interrupt — so the self-IPI is
/// skipped, but the flag is still published: a wake raised from IRQ context on
/// the CPU that is running the hog is exactly the case that must preempt.
pub(crate) fn request_resched(owner: u8) {
    let owner = owner as usize;
    if owner >= 64 {
        return;
    }
    let bit = 1u64 << owner;
    if !WAKEUP_PREEMPT.load(Ordering::Relaxed) {
        // Disabled: fall back to the pre-change behaviour — kick a CPU that is
        // halted so it leaves `hlt`, and leave a busy one to finish its slice.
        if SLEEPING_CPUS.load(Ordering::SeqCst) & bit != 0 {
            send_resched_ipi(owner);
        }
        return;
    }
    // The halt protocol is carried by the caller's `notify` (a SeqCst RMW) and
    // this SeqCst load of the sleeping mask: a wake that does not observe the
    // target as sleeping is guaranteed to be observed by that target's
    // pre-halt `has_ready()` recheck. `NEED_RESCHED` is a pure optimisation
    // hint layered on top, so its ordering does not enter that argument.
    let sleeping = SLEEPING_CPUS.load(Ordering::SeqCst) & bit != 0;
    // Relaxed pre-check keeps a burst of wakes off this shared cacheline: with
    // a request already outstanding there is nothing new to publish.
    if NEED_RESCHED.load(Ordering::Relaxed) & bit != 0 {
        // …but a target that parked *after* that earlier request still has to
        // be kicked out of `hlt`, or the wake waits for its next tick.
        if sleeping {
            send_resched_ipi(owner);
        }
        return;
    }
    let already_pending = NEED_RESCHED.fetch_or(bit, Ordering::SeqCst) & bit != 0;
    if !already_pending {
        RESCHED_REQUESTED.fetch_add(1, Ordering::Relaxed);
    }
    if sleeping || (!already_pending && owner != crate::arch::cpu_id() as usize) {
        send_resched_ipi(owner);
    }
}

/// Trap-path side: consume this CPU's pending wake-up preemption request.
///
/// Returns `true` exactly once per request, to the caller that should yield.
pub fn take_need_resched() -> bool {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= 64 {
        return false;
    }
    let bit = 1u64 << cpu;
    // Relaxed pre-check: the flag is clear on the overwhelming majority of
    // traps, and this runs on every user trap (timer tick, IPI, syscall-free
    // interrupt). Only pay for the RMW when there is something to take.
    if NEED_RESCHED.load(Ordering::Relaxed) & bit == 0 {
        return false;
    }
    let taken = NEED_RESCHED.fetch_and(!bit, Ordering::AcqRel) & bit != 0;
    if taken {
        RESCHED_TAKEN.fetch_add(1, Ordering::Relaxed);
    }
    taken
}

/// Executor-side: drop any pending request for this CPU.
///
/// Called when the run queue is found empty — there is by definition nothing
/// left to preempt *for*, and leaving the bit set would suppress the coalesced
/// IPI for the next genuine wake.
#[inline]
pub(crate) fn clear_need_resched(cpu: usize) {
    if cpu >= 64 {
        return;
    }
    let bit = 1u64 << cpu;
    // Relaxed pre-check: this runs on every pass of the idle loop (hundreds of
    // times a second per idle CPU) and the bit is almost always already clear.
    // An unconditional RMW would bounce this cacheline between every idle core.
    if NEED_RESCHED.load(Ordering::Relaxed) & bit != 0 {
        NEED_RESCHED.fetch_and(!bit, Ordering::Relaxed);
    }
}

pub struct ExecutorRuntime {
    // runtime only run on this cpu
    cpu_id: u8,

    // 只会在一个 core 上运行，不需要考虑同步问题
    task_collection: Arc<TaskCollection>,

    // 通过 force_switch_future 会将 strong_executor 降级为 weak_executor
    strong_executor: Arc<Pin<Box<Executor>>>,

    // 该 executor 在执行完一次后就会被 drop
    weak_executors: Vec<Option<Arc<Pin<Box<Executor>>>>>,

    // 当前正在执行的 executor
    current_executor: Option<Arc<Pin<Box<Executor>>>>,

    // runtime context, WARN: riscv and x86_64 use different struct
    context: Context,
}

impl ExecutorRuntime {
    pub fn new(cpu_id: u8) -> Self {
        let task_collection = TaskCollection::new(cpu_id);
        let tc_clone = task_collection.clone();
        ExecutorRuntime {
            cpu_id,
            task_collection,
            strong_executor: Arc::new(Executor::new(tc_clone)),
            weak_executors: vec![],
            current_executor: None,
            context: Context::default(),
        }
    }

    pub fn cpu_id(&self) -> u8 {
        self.cpu_id
    }

    pub(crate) fn weak_executor_num(&self) -> usize {
        self.weak_executors.len()
    }

    // return task number of current cpu.
    pub fn task_num(&self) -> usize {
        self.task_collection.task_num()
    }

    /// Number of tasks on this CPU that are runnable right now — the load
    /// figure used for placement and work stealing. `None` if the collection
    /// was momentarily locked (callers skip such a CPU rather than spin).
    pub(crate) fn ready_num(&self) -> Option<usize> {
        self.task_collection.ready_num()
    }

    fn add_weak_executor(&mut self, weak_executor: Arc<Pin<Box<Executor>>>) {
        self.weak_executors.push(Some(weak_executor));
    }

    fn downgrade_strong_executor(&mut self) {
        // SAFETY: 只会在一个 core 上运行，不需要考虑同步问题
        let mut old = self.strong_executor.clone();
        unsafe {
            Arc::get_mut_unchecked(&mut old).mark_weak();
        }
        self.add_weak_executor(old);
        self.strong_executor = Arc::new(Executor::new(self.task_collection.clone()));
    }

    // 添加一个task，它的初始状态是 notified，也就是说它可以被执行.
    fn add_task<F: Future<Output = ()> + 'static + Send>(
        &self,
        priority: usize,
        future: F,
        affinity: Option<Arc<AtomicU64>>,
    ) -> Key {
        debug_assert!(priority < MAX_PRIORITY);
        self.task_collection.add_task(future, affinity)
    }

    fn remove_task(&self, key: Key) {
        self.task_collection.remove_task(key)
    }

    #[cfg(target_arch = "riscv64")]
    fn get_context(&self) -> usize {
        &self.context as *const Context as usize
    }

    #[cfg(target_arch = "x86_64")]
    fn get_context(&self) -> usize {
        self.context.get_context()
    }

    #[cfg(target_arch = "aarch64")]
    fn get_context(&self) -> usize {
        &self.context as *const Context as usize
    }
}

impl Drop for ExecutorRuntime {
    fn drop(&mut self) {
        panic!("drop executor runtime!!!!");
    }
}

// SAFETY: 只会在一个 core 上运行，不需要考虑同步问题
unsafe impl Send for ExecutorRuntime {}
unsafe impl Sync for ExecutorRuntime {}

// TODO: more elegent?
lazy_static! {
    pub static ref GLOBAL_RUNTIME: [Mutex<ExecutorRuntime>; MAX_CORE_NUM] =
        core::array::from_fn(|i| Mutex::new(ExecutorRuntime::new(i as u8)));
}

// obtain a task from other cpu.
pub(crate) fn steal_task_from_other_cpu() -> Option<(Key, Arc<Task>, Arc<WakerRef>)> {
    let current_cpu = crate::arch::cpu_id() as usize;
    // Use try_lock() so that idle CPUs never spin-wait on each other's runtime
    // locks during the scan phase.  On a many-core machine this prevents the
    // O(N) blocking-lock storm that otherwise occurs when N-1 idle CPUs
    // simultaneously try to steal work.  A locked runtime is by definition
    // actively being used; we simply skip it and try again next scheduling
    // cycle rather than queue-spinning on its lock.
    //
    // Affinity is enforced inside `take_task` (its generator skips tasks not
    // allowed on the current CPU), so a victim with the most tasks may still
    // yield nothing for us. We therefore consider every non-empty victim,
    // most-loaded first, and try them in turn until one hands us a runnable
    // task — instead of giving up after probing a single busiest CPU.
    //
    // "Most loaded" means the most *runnable* tasks, not the most tasks: a CPU
    // whose collection is full of blocked daemons has nothing to give, and
    // ranking it first pushed the genuinely backlogged CPU to the end of the
    // probe order (or past the `try_lock` failures that end the scan).
    //
    // Fixed-size stack buffer: this runs on every idle scheduling pass, where
    // the previous `Vec` was a heap allocation per iteration.
    let mut candidates = [(0usize, 0usize); MAX_CORE_NUM];
    let mut n = 0;
    for (i, runtime_mutex) in GLOBAL_RUNTIME.iter().enumerate().take(num_online_cpus()) {
        if i == current_cpu {
            // Never steal from ourselves; our own collection is already empty.
            continue;
        }
        if let Some(runtime) = runtime_mutex.try_lock() {
            // `ready_num() == None` means the collection was locked; skip it
            // this pass rather than spin (see the deadlock discipline below).
            if let Some(count) = runtime.ready_num() {
                if count > 0 {
                    candidates[n] = (i, count);
                    n += 1;
                }
            }
        }
    }
    // Most-loaded victims first to spread work off the busiest cores.
    candidates[..n].sort_unstable_by(|a, b| b.1.cmp(&a.1));
    for &(cpu, _) in &candidates[..n] {
        // Deadlock discipline: the thief may hold the victim's runtime lock
        // while stealing (that serialization keeps the victim's executor
        // transitions — which run under lazily-retained user CR3s — from
        // overlapping with thief-driven task drops on the victim collection),
        // but it must NEVER SPIN while holding it. The AB-BA otherwise: thief
        // holds RUNTIME[victim] and spins on the victim's generator lock; the
        // victim's executor holds that generator (mid take_task) and is
        // interrupted by a timer IRQ whose sched_yield spins on
        // RUNTIME[victim] with interrupts off -> the executor can never
        // resume to release the generator -> both CPUs spin forever (the >8s
        // DEADLOCK banner). Hence try_lock on BOTH locks: a busy victim is
        // skipped, never waited on.
        let Some(runtime) = GLOBAL_RUNTIME[cpu].try_lock() else {
            continue;
        };
        if runtime.task_num() > 0 {
            if let Some(task) = runtime.task_collection.try_take_task() {
                return Some(task);
            }
        }
    }
    None
}

/// Load of `cpu` for placement purposes: runnable tasks if the collection can
/// be inspected without blocking, otherwise the total task count as a
/// conservative stand-in (a locked collection is by definition in use).
fn placement_load(runtime: &ExecutorRuntime) -> usize {
    runtime.ready_num().unwrap_or_else(|| runtime.task_num())
}

// per-cpu scheduler.
pub fn run_until_idle() -> bool {
    debug!("GLOBAL_RUNTIME.run()");
    // Make this CPU eligible for task placement and work stealing.
    NUM_ONLINE_CPUS.fetch_max(crate::arch::cpu_id() as usize + 1, Ordering::Relaxed);
    loop {
        let mut runtime = get_current_runtime();
        let runtime_cx = runtime.get_context();
        let executor_cx = runtime.strong_executor.context.get_context();
        debug!("switch idle -> {}", runtime.strong_executor.id());
        runtime.current_executor = Some(runtime.strong_executor.clone());
        // 释放保护 global_runtime 的锁
        drop(runtime);
        debug!("run strong executor");
        switch(runtime_cx, executor_cx);
        // 该函数返回说明当前 strong_executor 执行的 future 超时或者主动 yield 了,
        // 需要重新创建一个 executor 执行后续的 future, 并且将
        // 新的 executor 作为 strong_executor，旧的 executor 添
        // 加到 weak_exector 中。
        runtime = get_current_runtime();
        runtime.current_executor = None;
        if cfg!(feature = "baremetal-test") && runtime.task_num() == 0 {
            return false;
        }
        // 只有 strong_executor 主动 yield 时, 才会执行运行 weak_executor;
        if runtime.strong_executor.is_running_future() {
            runtime.downgrade_strong_executor();
            continue;
        }
        // 遍历全部的 weak_executor
        if runtime.weak_executors.is_empty() {
            drop(runtime);
            continue;
        }
        debug!("run weak executor");
        runtime
            .weak_executors
            .retain(|executor| executor.is_some() && !executor.as_ref().unwrap().killed());
        for idx in 0..runtime.weak_executors.len() {
            if let Some(executor) = &runtime.weak_executors[idx] {
                if executor.killed() {
                    continue;
                }
                let executor = executor.clone();
                let executor_ctx = executor.context.get_context();
                debug!("switch idle -> {}", executor.id());
                runtime.current_executor = Some(executor);
                drop(runtime);
                switch(runtime_cx as _, executor_ctx as _);
                runtime = get_current_runtime();
                runtime.current_executor = None;
            }
        }
    }
}

pub fn spawn(future: impl Future<Output = ()> + Send + 'static) {
    super::run_with_intr_saved_off! {
        // Distribute tasks to the least-loaded CPU rather than pinning all new
        // work to the spawner's CPU.  spawn_task with cpu_id=None picks the
        // least-loaded CPU via a non-blocking try_lock scan, which is cheap and
        // keeps all cores busy on a many-core machine.
        spawn_task(future, None, None, None)
    }
}

/// Spawn a coroutine carrying a CPU affinity mask.
///
/// The task will only ever be placed on, and stolen by, CPUs whose bit is set
/// in `affinity`. The mask is shared (`Arc`) so the owning thread can update it
/// later via `sched_setaffinity`; the scheduler re-reads it on every placement
/// and steal decision.
pub fn spawn_with_affinity(
    future: impl Future<Output = ()> + Send + 'static,
    affinity: Arc<AtomicU64>,
) {
    super::run_with_intr_saved_off! {
        spawn_task(future, None, None, Some(affinity))
    }
}

/// Spawn a coroutine with `priority`, `cpu_id` and an optional affinity mask.
/// Default priority: DEFAULT_PRIORITY
/// Default cpu_id: the least-loaded CPU allowed by `affinity`
pub fn spawn_task(
    future: impl Future<Output = ()> + Send + 'static,
    priority: Option<usize>,
    cpu_id: Option<usize>,
    affinity: Option<Arc<AtomicU64>>,
) {
    debug!("try to spawn {:?} {:?}", priority, cpu_id);
    let priority = priority.unwrap_or(DEFAULT_PRIORITY);
    let (runtime, target_cpu) = if let Some(cpu_id) = cpu_id {
        assert!(
            cpu_id < MAX_CORE_NUM,
            "spawn_task: cpu_id {} out of range (MAX_CORE_NUM={})",
            cpu_id,
            MAX_CORE_NUM
        );
        (&GLOBAL_RUNTIME[cpu_id], cpu_id)
    } else {
        // Use try_lock() to find the least-loaded online CPU without stalling
        // callers. If a runtime is currently locked (busy), we skip it and
        // consider the others; the CPU whose runtime is unlocked and has the
        // fewest *runnable* tasks wins (see `placement_load`: counting parked
        // tasks made a CPU full of sleeping daemons look busier than a CPU
        // saturated with spinning threads, which is backwards). Only CPUs
        // allowed by the affinity mask are considered, so an affine task is
        // born on a legal CPU instead of being placed anywhere and then bounced
        // off by the affinity check.
        let online = num_online_cpus();
        let mask = affinity
            .as_ref()
            .map(|a| a.load(Ordering::Relaxed))
            .unwrap_or(u64::MAX);
        // First CPU allowed by the mask, used as the fallback home when every
        // candidate runtime is momentarily locked.
        let mut best = (0..online).find(|&i| (mask >> i) & 1 != 0).unwrap_or(0);
        let mut best_count = usize::MAX;
        for (i, rt) in GLOBAL_RUNTIME.iter().enumerate().take(online) {
            if (mask >> i) & 1 == 0 {
                continue;
            }
            if let Some(rt) = rt.try_lock() {
                let count = placement_load(&rt);
                if count < best_count {
                    best_count = count;
                    best = i;
                }
            }
        }
        (&GLOBAL_RUNTIME[best], best)
    };
    crate::diag::diag_lock(runtime).add_task(priority, future, affinity);
    // A new task is born `notified` on `target_cpu`'s queue, which is the same
    // situation as a wake: if that CPU is halted it would not notice until its
    // next tick, and if it is busy the new task would not get to run until the
    // occupying thread's timeslice expired. A freshly `fork`ed child waiting out
    // 20 ms before its first instruction is a large part of why launching a
    // command feels slow, so it gets the same treatment as any other wake.
    request_resched(target_cpu as u8);
}

/// check whether the running coroutine of current cpu time out, if yes, we will
/// switch to currrent cpu runtime that would create a new executor to run other
/// coroutines.
pub fn handle_timeout() {
    debug!("handle kernel timeout");
    super::run_with_intr_saved_off! {
        sched_yield()
    }
}

/// 运行executor.run()
#[no_mangle]
pub(crate) fn run_executor(executor_addr: usize) {
    let mut p = unsafe { Box::from_raw(executor_addr as *mut Executor) };
    p.run();
    // Weak executor may return
    let runtime = get_current_runtime();
    let executor_cx = p.context.get_context();
    let runtime_cx = runtime.get_context();
    debug!("executor all done! switch {} -> idle", p.id());
    drop(runtime);
    switch(executor_cx as _, runtime_cx as _);
    unreachable!();
}

/// [diag] Timer-IRQ hook: panic loudly if the currently-running executor's
/// stack canary has been clobbered.
///
/// The coroutine stacks are guard-page-less heap allocations; an overflow
/// (runaway recursion, oversized stack frame chain) writes silently into
/// neighbouring heap allocations and surfaces later as wild corruption that is
/// near-impossible to attribute (the `/proc/self/exe` self-reference bug cost
/// exactly that hunt — see docs/README-crash-repro.md). The canary at the
/// stack base is written at executor creation; any deep overflow passes
/// through it, so a per-tick check bounds the damage window to ~4 ms and
/// names the executor instead of leaving mystery corruption.
///
/// `try_lock` everywhere: this runs in hard IRQ context — if the interrupted
/// code holds the runtime lock we simply skip this tick's check.
pub fn check_current_executor_canary() {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CORE_NUM {
        return;
    }
    let Some(rt) = GLOBAL_RUNTIME[cpu].try_lock() else {
        return;
    };
    let Some(ex) = rt.current_executor.as_ref() else {
        return;
    };
    if !ex.canary_intact() {
        let (id, base, task) = (ex.id(), ex.stack_base(), ex.task_id());
        drop(rt);
        panic!(
            "\n[stack-canary] COROUTINE STACK OVERFLOW on CPU{}: executor id={} \
             task_id={} stack_base={:#x} — canary clobbered; a kernel call chain \
             ran off the guard-page-less coroutine stack into the heap\n",
            cpu, id, task, base,
        );
    }
}

/// switch to runtime, which would select an appropriate executor to run.
pub fn sched_yield() {
    let runtime = get_current_runtime();
    if let Some(executor) = runtime.current_executor.as_ref() {
        let executor_cx = executor.context.get_context();
        debug!("switch {} -> idle", executor.id());
        let runtime_cx = runtime.get_context();
        drop(runtime);
        switch(executor_cx, runtime_cx);
    }
}

pub(crate) fn switch(from_ctx: usize, to_ctx: usize) {
    unsafe {
        crate::arch::switch(from_ctx as _, to_ctx as _);
    }
}

/// return runtime `MutexGuard` of current cpu.
pub(crate) fn get_current_runtime() -> MutexGuard<'static, ExecutorRuntime> {
    let id = crate::arch::cpu_id() as usize;
    assert!(
        id < MAX_CORE_NUM,
        "cpu_id {} out of range (MAX_CORE_NUM={})",
        id,
        MAX_CORE_NUM
    );
    crate::diag::diag_lock(&GLOBAL_RUNTIME[id])
}

#[allow(dead_code)]
// Just for debug
pub fn get_current_executor_id() -> (usize, usize) {
    let runtime = get_current_runtime();
    if let Some(executor) = runtime.current_executor.as_ref() {
        (executor.id(), executor.task_id())
    } else {
        (0, 0)
    }
}
