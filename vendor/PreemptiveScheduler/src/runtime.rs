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

/// Bitmask of CPUs that have entered [`run_until_idle`] (executor ready).
/// Placement, steal and affinity use this — NOT `max(id)+1`, which could mark
/// holes as online when a higher AP starts before a lower one.
static EXECUTOR_READY: AtomicU64 = AtomicU64::new(1); // BSP bit for early spawn

#[inline]
pub(crate) fn executor_ready_mask() -> u64 {
    EXECUTOR_READY.load(Ordering::Acquire)
}

#[inline]
pub(crate) fn is_executor_ready(cpu: usize) -> bool {
    cpu < 64 && (executor_ready_mask() & (1u64 << cpu)) != 0
}

#[inline]
fn mark_executor_ready(cpu: usize) {
    if cpu < 64 {
        EXECUTOR_READY.fetch_or(1u64 << cpu, Ordering::Release);
    }
}

/// One past the highest ready logical cpu id (for `take(n)` style scans).
/// Prefer [`is_executor_ready`] / [`executor_ready_mask`] when skipping holes.
#[inline]
pub(crate) fn num_online_cpus() -> usize {
    let m = executor_ready_mask();
    if m == 0 {
        return 1;
    }
    (64 - m.leading_zeros()) as usize
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

/// Waker-forwarding: a notified task cannot run on `skip` (affinity), so make
/// one of its ALLOWED CPUs look for it. Sleeping CPUs first — they answer in
/// IPI time and cost nothing to wake — else the lowest allowed, whose
/// `request_resched` publication coalesces with any outstanding request.
pub(crate) fn kick_for_affinity(mask: u64, skip: usize) {
    let mask = if skip < 64 { mask & !(1u64 << skip) } else { mask };
    if mask == 0 {
        return;
    }
    let sleeping = SLEEPING_CPUS.load(Ordering::SeqCst) & mask;
    let target = if sleeping != 0 {
        sleeping.trailing_zeros()
    } else {
        mask.trailing_zeros()
    } as u8;
    request_resched(target);
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

    pub(crate) fn placement_load(&self) -> Option<usize> {
        self.task_collection.placement_load()
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

/// Force construction of every per-CPU [`ExecutorRuntime`] (and thus the first
/// `Executor::new` per core) *after* [`crate::set_stack_guard_hooks`] so hard
/// guards are installed. Call from bare-metal boot immediately after
/// `stack_guard::init`. Idempotent.
pub fn warm_runtimes() {
    let _ = &*GLOBAL_RUNTIME;
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
        if i == current_cpu || !is_executor_ready(i) {
            // Never steal from ourselves; skip CPUs that never entered the executor.
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
    runtime
        .placement_load()
        .unwrap_or_else(|| runtime.task_num())
}

// per-cpu scheduler.
pub fn run_until_idle() -> bool {
    debug!("GLOBAL_RUNTIME.run()");
    // Make this CPU eligible for task placement and work stealing.
    mark_executor_ready(crate::arch::cpu_id() as usize);
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
        // Prefer an executor-ready CPU; fall back only if explicitly requested.
        if !is_executor_ready(cpu_id) {
            warn!(
                "spawn_task: cpu_id {} not executor-ready yet — task may stall until steal",
                cpu_id
            );
        }
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
        // First ready CPU allowed by the mask, used as the fallback home when every
        // candidate runtime is momentarily locked.
        let mut best = (0..online)
            .find(|&i| is_executor_ready(i) && (mask >> i) & 1 != 0)
            .unwrap_or(0);
        let mut best_count = usize::MAX;
        for (i, rt) in GLOBAL_RUNTIME.iter().enumerate().take(online) {
            if !is_executor_ready(i) || (mask >> i) & 1 == 0 {
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

/// [diag] IRQ hook: panic loudly if the currently-running executor's
/// RSP has grown within `STACK_LOW_WATER_MARK` of its stack base.
///
/// The canary check (`check_current_executor_canary`) only fires once the
/// very bottom of the stack allocation is overwritten, by which point the
/// heap below it is already corrupted. Checking RSP proximity catches a
/// near-overflow *before* any heap object is touched: if the faulting frame
/// in the new requirement (lunarbar null-READ, `[rsp0]=0x13406`) was produced
/// by a stack that had not yet touched the canary, this check would have
/// panicked cleanly on a preceding IRQ.
///
/// Only meaningful when `rsp` is an executor kernel stack pointer (caller
/// should pass a ring-0 trap-frame RSP). Under `hard_guard`,
/// `canary_intact` is always true — this proximity check is the early
/// warning before the unmapped guard #PF.
///
/// When `current_executor` is `None` (idle), still tests whether `rsp` lies
/// on any live strong/weak executor stack on this CPU.
///
/// `try_lock` everywhere: runs in hard IRQ context.
pub fn check_current_executor_stack_proximity(rsp: usize) {
    /// Panic threshold: remaining depth below this is seconds from a silent
    /// heap clobber. Raised 32→128 KiB after labwc/lunarbar still smashed
    /// past the old mark between 4 ms timer samples (`[rsp0]=0x13446`).
    const STACK_LOW_WATER_MARK: usize = 128 * 1024; // 128 KiB
    // Keep in sync with `executor::{STACK_SIZE, GUARD_SIZE, TOP_GUARD_SIZE}`.
    use crate::{GUARD_SIZE, STACK_SIZE, TOP_GUARD_SIZE};
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CORE_NUM {
        return;
    }
    let Some(rt) = GLOBAL_RUNTIME[cpu].try_lock() else {
        return;
    };
    let in_danger = |base: usize| -> bool {
        let stack_top = base + STACK_SIZE;
        // Past the usable base into the bottom guard / neighbour heap: RSP
        // already escaped the stack. The hard-guard #PF only fires if the
        // access touches unmapped pages — a large `sub rsp` can skip them;
        // catch that here before a later IRQ detonates as rip=0 / [rsp0]=0x13446.
        if rsp < base && rsp + GUARD_SIZE >= base {
            return true;
        }
        // RSP landed exactly on the base edge (first usable byte / last guard).
        if rsp == base {
            return true;
        }
        // Past the usable top into the TOP_GUARD (neighbour growing down).
        if rsp >= stack_top && rsp < stack_top + TOP_GUARD_SIZE {
            return true;
        }
        // Low-water inside the usable region.
        rsp > base && rsp < stack_top && rsp - base < STACK_LOW_WATER_MARK
    };
    // Prefer the running executor; if idle, still test whether `rsp` sits on
    // any live executor stack (IRQ nesting after yield left current=None).
    let hit: Option<(usize, usize, usize)> = if let Some(ex) = rt.current_executor.as_ref() {
        in_danger(ex.stack_base()).then(|| (ex.id(), ex.task_id(), ex.stack_base()))
    } else {
        let mut found = None;
        if in_danger(rt.strong_executor.stack_base()) {
            found = Some((
                rt.strong_executor.id(),
                rt.strong_executor.task_id(),
                rt.strong_executor.stack_base(),
            ));
        } else {
            for slot in rt.weak_executors.iter() {
                if let Some(ex) = slot {
                    if in_danger(ex.stack_base()) {
                        found = Some((ex.id(), ex.task_id(), ex.stack_base()));
                        break;
                    }
                }
            }
        }
        found
    };
    if let Some((id, task, base)) = hit {
        drop(rt);
        let remaining = rsp.saturating_sub(base);
        panic!(
            "\n[stack-proximity] COROUTINE STACK NEAR OVERFLOW on CPU{}: executor id={} \
             task_id={} stack_base={:#x} rsp={:#x} remaining={} bytes — \
             less than {} KiB left / in bottom-guard or top-guard VA range\n",
            cpu,
            id,
            task,
            base,
            rsp,
            remaining,
            STACK_LOW_WATER_MARK / 1024,
        );
    }
}

/// Whether hard-IRQ work that walks heap-backed trait objects (xHCI
/// `EventListener::trigger`, graphic present, …) should be skipped this tick.
///
/// IRQs nest on the coroutine stack. When less than 128 KiB remains, another
/// `Box<dyn Fn>` dispatch is how desktop bring-up turned a near-overflow into
/// `rip=0x0` / `[rsp0]=0x13446` with `no current thread` on a later idle tick.
#[inline]
pub fn irq_should_skip_heavy_work() -> bool {
    /// Below this remaining depth, refuse HID poll / other deep IRQ work so
    /// the (soft or hard) guard stays intact. Raised from 64→128 KiB after
    /// labwc bring-up still smashed with the lower threshold.
    const HEAVY_SKIP_REMAINING: usize = 128 * 1024;
    #[cfg(target_arch = "x86_64")]
    let rsp: usize = {
        let mut rsp: usize;
        // SAFETY: reading RSP only; no memory side effects.
        unsafe {
            core::arch::asm!(
                "mov {}, rsp",
                out(reg) rsp,
                options(nostack, nomem, preserves_flags)
            );
        }
        rsp
    };
    #[cfg(not(target_arch = "x86_64"))]
    let rsp: usize = 0;
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = rsp;
        return false;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CORE_NUM {
        return false;
    }
    let Some(rt) = GLOBAL_RUNTIME[cpu].try_lock() else {
        return false;
    };
    /// Same usable size as `executor::STACK_SIZE` (re-export).
    use crate::STACK_SIZE;
    let near = |base: usize| -> bool {
        rsp > base && rsp < base + STACK_SIZE && rsp.saturating_sub(base) < HEAVY_SKIP_REMAINING
    };
    // Prefer current; if idle (None), still skip heavy work when RSP sits in
    // the low-water zone of any live executor — that was the blind spot behind
    // `no current thread` + rip=0 after labwc smash.
    if let Some(ex) = rt.current_executor.as_ref() {
        return near(ex.stack_base());
    }
    if near(rt.strong_executor.stack_base()) {
        return true;
    }
    for slot in rt.weak_executors.iter() {
        if let Some(ex) = slot {
            if near(ex.stack_base()) {
                return true;
            }
        }
    }
    false
}

/// Sticky soft-smash / heap-corruption flag **per CPU**. A smash on one core
/// must not disable dyn dispatch on every other core (desktop limp).
static HEAP_SMASH_SUSPECTED: [AtomicBool; MAX_CORE_NUM] =
    [const { AtomicBool::new(false) }; MAX_CORE_NUM];

/// Record that a soft-smash / smashed fat-ptr was observed (trap or diag).
#[inline]
pub fn note_heap_smash_suspected() {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < MAX_CORE_NUM {
        HEAP_SMASH_SUSPECTED[cpu].store(true, Ordering::Relaxed);
    }
}

/// Whether this CPU has already suspected a heap smash this boot.
#[inline]
pub fn heap_smash_suspected() -> bool {
    let cpu = crate::arch::cpu_id() as usize;
    cpu < MAX_CORE_NUM && HEAP_SMASH_SUSPECTED[cpu].load(Ordering::Relaxed)
}

/// Where a fault pointer sits relative to one executor's stack layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackPtrRegion {
    /// `[stack_base, stack_base + STACK_SIZE)` — usable coroutine stack.
    Usable,
    /// `[stack_base - GUARD_SIZE, stack_base)` — bottom hard/soft guard VA.
    BottomGuard,
    /// `[stack_base + STACK_SIZE, stack_base + STACK_SIZE + TOP_GUARD_SIZE)`.
    TopGuard,
}

impl StackPtrRegion {
    pub fn as_str(self) -> &'static str {
        match self {
            StackPtrRegion::Usable => "usable",
            StackPtrRegion::BottomGuard => "bottom-guard",
            StackPtrRegion::TopGuard => "top-guard",
        }
    }
}

/// One hit from [`attribute_fault_stack_ptrs`]: `addr` falls on this executor.
#[derive(Clone, Copy, Debug)]
pub struct StackAttrHit {
    pub cpu: usize,
    pub executor_id: usize,
    pub task_id: usize,
    pub stack_base: usize,
    pub region: StackPtrRegion,
}

/// Result of walking every executor (try_lock) for fault `rsp` / `rbp`.
#[derive(Clone, Copy, Debug)]
pub struct FaultStackAttr {
    pub rsp: Option<StackAttrHit>,
    pub rbp: Option<StackAttrHit>,
    /// CPUs whose runtime mutex was locked (skipped).
    pub cpus_skipped: usize,
    /// CPUs successfully walked.
    pub cpus_walked: usize,
    /// Executors examined across walked CPUs.
    pub executors_seen: usize,
}

fn classify_stack_ptr(addr: usize, base: usize) -> Option<StackPtrRegion> {
    use crate::{GUARD_SIZE, STACK_SIZE, TOP_GUARD_SIZE};
    if addr >= base && addr < base + STACK_SIZE {
        Some(StackPtrRegion::Usable)
    } else if addr >= base.saturating_sub(GUARD_SIZE) && addr < base {
        Some(StackPtrRegion::BottomGuard)
    } else if addr >= base + STACK_SIZE && addr < base + STACK_SIZE + TOP_GUARD_SIZE {
        Some(StackPtrRegion::TopGuard)
    } else {
        None
    }
}

/// Walk all executors on all CPUs (`try_lock` only) and report whether fault
/// `rsp` / `rbp` fall in usable stack, bottom/top guard VA, or outside all.
///
/// Proves soft-smash attribution: truncated `[rsp]=0x13446` with hard guards
/// installed and no `[stack-guard] #PF` means either an in-stack buffer smash
/// (rsp still in usable) or heap/fn-ptr corruption (rsp outside every stack).
pub fn attribute_fault_stack_ptrs(rsp: usize, rbp: usize) -> FaultStackAttr {
    let mut out = FaultStackAttr {
        rsp: None,
        rbp: None,
        cpus_skipped: 0,
        cpus_walked: 0,
        executors_seen: 0,
    };
    let online = num_online_cpus().min(MAX_CORE_NUM);
    for cpu in 0..online {
        let Some(rt) = GLOBAL_RUNTIME[cpu].try_lock() else {
            out.cpus_skipped += 1;
            continue;
        };
        out.cpus_walked += 1;
        let mut consider = |base: usize, id: usize, task: usize| {
            out.executors_seen += 1;
            if out.rsp.is_none() {
                if let Some(region) = classify_stack_ptr(rsp, base) {
                    out.rsp = Some(StackAttrHit {
                        cpu,
                        executor_id: id,
                        task_id: task,
                        stack_base: base,
                        region,
                    });
                }
            }
            if out.rbp.is_none() {
                if let Some(region) = classify_stack_ptr(rbp, base) {
                    out.rbp = Some(StackAttrHit {
                        cpu,
                        executor_id: id,
                        task_id: task,
                        stack_base: base,
                        region,
                    });
                }
            }
        };
        consider(
            rt.strong_executor.stack_base(),
            rt.strong_executor.id(),
            rt.strong_executor.task_id(),
        );
        for slot in rt.weak_executors.iter() {
            if let Some(ex) = slot {
                consider(ex.stack_base(), ex.id(), ex.task_id());
            }
        }
        // Both found — no need to walk further CPUs.
        if out.rsp.is_some() && out.rbp.is_some() {
            break;
        }
    }
    out
}

/// True when there is no current executor on this CPU (idle IRQ path).
#[inline]
pub fn irq_on_idle_executor() -> bool {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CORE_NUM {
        return true;
    }
    let Some(rt) = GLOBAL_RUNTIME[cpu].try_lock() else {
        // Lock held — treat as unknown; do not force-skip.
        return false;
    };
    rt.current_executor.is_none()
}

/// IRQ nest on an executor stack whose current `[RSP]` qword is null.
///
/// Historically `executor_entry` did `push 0; jmp run_executor`, leaving a
/// null return pad; RET through it was null-EXECUTE. Entry now uses `call`
/// (real return). A smashed / still-zero slot matches too.
#[inline]
pub fn current_stack_top_looks_null() -> bool {
    #[cfg(not(target_arch = "x86_64"))]
    {
        return false;
    }
    #[cfg(target_arch = "x86_64")]
    {
        let rsp: usize = {
            let mut rsp: usize;
            // SAFETY: reading RSP only.
            unsafe {
                core::arch::asm!(
                    "mov {}, rsp",
                    out(reg) rsp,
                    options(nostack, nomem, preserves_flags)
                );
            }
            rsp
        };
        if (rsp & 7) != 0 {
            return false;
        }
        if !(rsp >= 0xffff_ff00_0000_0000 && rsp < 0xffff_ff01_0000_0000) {
            return false;
        }
        // SAFETY: rsp 8-aligned in kernel stack window.
        let top = unsafe { core::ptr::read_volatile(rsp as *const u64) };
        if top != 0 {
            return false;
        }
        // Confirm we are on this CPU's executor usable (or guard) region so a
        // null qword on an unrelated kernel stack does not trip the skip.
        let cpu = crate::arch::cpu_id() as usize;
        if cpu >= MAX_CORE_NUM {
            return true;
        }
        let Some(rt) = GLOBAL_RUNTIME[cpu].try_lock() else {
            // Lock held + [rsp]==0: be conservative.
            return true;
        };
        use crate::STACK_SIZE;
        let on_stack = |base: usize| -> bool {
            classify_stack_ptr(rsp, base).is_some()
                || (rsp > base && rsp < base + STACK_SIZE)
        };
        if on_stack(rt.strong_executor.stack_base()) {
            return true;
        }
        for slot in rt.weak_executors.iter() {
            if let Some(ex) = slot {
                if on_stack(ex.stack_base()) {
                    return true;
                }
            }
        }
        // Idle runtime context with [rsp]==0 — still dangerous for dyn.
        // (Do not call irq_on_idle_executor(): we already hold `rt`.)
        rt.current_executor.is_none()
    }
}

/// Skip *all* heap-backed dyn dispatch from `timer_tick` / idle IRQ:
/// near-overflow on the current coroutine, OR soft-smash / fat-ptr smash
/// already sticky-noted (do not keep calling through a half-smashed heap).
///
/// Idle path also skips when `[RSP]==0` on the current stack (null return /
/// alignment zero that would RET to rip=0).
#[inline]
pub fn irq_should_skip_dyn_dispatch() -> bool {
    if irq_should_skip_heavy_work() {
        return true;
    }
    // Once smash is suspected, skip every tick — not only idle. Continuing
    // timer `Box<dyn Fn>` work after `[rsp]=0x13446` is how a second #PF
    // cascades inside the same callback pass.
    if heap_smash_suspected() {
        return true;
    }
    // Idle IRQ: null stack top → skip ALL dyn (cursor / HID / Box callbacks).
    // `heap_smash_suspected` already covered above; drivers' sticky is OR'd by
    // the timer caller. Here we catch the pre-smash `[rsp]==0` pattern.
    if irq_on_idle_executor() && current_stack_top_looks_null() {
        return true;
    }
    false
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
/// Also scans strong + weak executors when `current_executor` is `None`
/// (idle): that was the blind spot behind `no current thread` + smash residue
/// after labwc/lunarbar overflowed on a previous tick.
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
    // Idle / between polls: still verify every live executor on this CPU.
    if rt.current_executor.is_none() {
        if !rt.strong_executor.canary_intact() {
            let id = rt.strong_executor.id();
            drop(rt);
            panic!(
                "\n[stack-canary] COROUTINE STACK OVERFLOW on CPU{}: strong executor id={} \
                 (idle — canary clobbered; heap may already be corrupt)\n",
                cpu, id,
            );
        }
        for slot in rt.weak_executors.iter() {
            if let Some(ex) = slot {
                if !ex.canary_intact() {
                    let id = ex.id();
                    drop(rt);
                    panic!(
                        "\n[stack-canary] COROUTINE STACK OVERFLOW on CPU{}: weak executor id={} \
                         (idle — canary clobbered; heap may already be corrupt)\n",
                        cpu, id,
                    );
                }
            }
        }
        return;
    }
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

/// The stack pointer of the caller's frame.
#[inline(always)]
fn current_sp() -> usize {
    let sp: usize;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack, preserves_flags))
    };
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("mv {}, sp", out(reg) sp, options(nomem, nostack, preserves_flags))
    };
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("mov {}, sp", out(reg) sp, options(nomem, nostack, preserves_flags))
    };
    sp
}

/// Whether the coroutine running on this CPU can be abandoned right now.
///
/// True only when this CPU is inside `Executor::run`'s `task.poll`, standing on
/// that executor's own stack, with the stack's overflow canary still intact.
/// The last condition matters: a canary that is already clobbered means the
/// neighbouring heap has been overwritten, and no amount of task-level recovery
/// makes such a kernel safe to keep running.
///
/// Answers `false` rather than blocking if this CPU's runtime is locked — the
/// callers are panic paths, where spinning on a lock is how a diagnosable fault
/// turns into a silent freeze.
pub fn current_task_abandonable() -> bool {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CORE_NUM {
        return false;
    }
    let Some(runtime) = GLOBAL_RUNTIME[cpu].try_lock() else {
        return false;
    };
    let Some(executor) = runtime.current_executor.as_ref() else {
        return false;
    };
    executor.is_polling() && executor.stack_contains(current_sp()) && executor.canary_intact()
}

/// Abandon the coroutine running on this CPU and hand the core back to the
/// scheduler. **Does not return** when it succeeds.
///
/// This is the escape hatch behind `zcore::oops`: a kernel fault taken while
/// servicing one task should cost that task, not the machine. The task is
/// retired (never polled again, its future leaked — see `Task::abandon`) and
/// the executor is marked killed, so when the runtime regains control it
/// replaces this executor with a fresh one and drops the abandoned stack
/// instead of ever resuming it. Everything the aborted call chain owned —
/// heap allocations, `Arc` references, lock guards it will now never release —
/// leaks with it. That is the deliberate trade: bounded leakage per contained
/// fault, in exchange for the other processes on the machine surviving.
///
/// Returns `false` (having changed nothing) when the coroutine cannot be
/// abandoned, so the caller can fall back to halting.
///
/// # Safety
///
/// The caller must be a fault/panic path standing on the coroutine's own stack,
/// with interrupts disabled, holding no kernel lock, and it must not touch any
/// state owned by the abandoned call chain afterwards.
/// Whether a fault on this CPU can be isolated by discarding the whole
/// executor (the idle/scheduler case, where there is no task to blame).
///
/// True when the faulting frames really are on the current executor's own
/// stack and it is NOT inside a poll. The canary is deliberately NOT required
/// here: this path exists for a stack corrupted from OUTSIDE (the recurring SMP
/// smash), and demanding an intact canary would decline exactly the case it is
/// meant to rescue. Nothing on the stack is trusted — it is discarded, not
/// resumed.
pub fn current_executor_abandonable() -> bool {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CORE_NUM {
        return false;
    }
    let Some(runtime) = GLOBAL_RUNTIME[cpu].try_lock() else {
        return false;
    };
    let Some(executor) = runtime.current_executor.as_ref() else {
        return false;
    };
    !executor.is_polling() && executor.stack_contains(current_sp())
}

/// Discard the executor running on this CPU and hand the core back to the
/// scheduler. **Does not return** when it succeeds.
///
/// The idle half of [`abandon_current_task`]: a null-range fault taken on an
/// executor's stack *between* polls has no future to retire, so the task path
/// refuses and the machine used to halt — even though nothing was in flight and
/// the core was recoverable. Here the executor itself is retired: `killed()`
/// becomes true so the runtime drops it instead of resuming its corrupt stack,
/// and `force_replace` makes the runtime build a fresh one rather than switching
/// straight back in.
///
/// # Safety
///
/// Must be called from the fault path on the faulting CPU, with interrupts off
/// and no kernel lock held, standing on the executor's own stack.
/// Whether the executor on this CPU owns the stack `fault_sp` points into.
///
/// For faults taken on a **different** stack than the one that failed: a `#DF`
/// or `#GP` is delivered on its own IST stack, so `current_sp()` says nothing
/// about which coroutine died. The arch trap entry stashes the faulting frame
/// (`kstats::note_fault_regs`) and this answers from that instead.
pub fn fault_sp_abandonable(fault_sp: usize) -> bool {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CORE_NUM {
        return false;
    }
    let Some(runtime) = GLOBAL_RUNTIME[cpu].try_lock() else {
        return false;
    };
    let Some(executor) = runtime.current_executor.as_ref() else {
        return false;
    };
    executor.stack_contains(fault_sp)
}

/// Discard the executor that owns `fault_sp` and hand the core back to the
/// scheduler. **Does not return** when it succeeds.
///
/// Unlike [`abandon_current_executor`] this does NOT require standing on the
/// dying stack, because the caller may be on an IST stack (`#DF`/`#GP`). The
/// switch saves our current SP into the abandoned executor's context slot —
/// harmless, since that executor is never resumed — and loads the runtime's,
/// which lives on its own stack. Leaving the IST stack mid-use is fine: the CPU
/// resets it to its top on every exception entry.
///
/// # Safety
///
/// Fault path, interrupts off, no kernel lock held. `fault_sp` must be the SP
/// captured from the faulting frame.
pub unsafe fn abandon_executor_for_sp(fault_sp: usize) -> bool {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CORE_NUM {
        return false;
    }
    let Some(runtime) = GLOBAL_RUNTIME[cpu].try_lock() else {
        return false;
    };
    let Some(executor) = runtime.current_executor.clone() else {
        return false;
    };
    if !executor.stack_contains(fault_sp) {
        return false;
    }
    // Retire whatever it was doing: a task if one was in flight, else the
    // executor itself. Either way it must never be resumed.
    if !executor.abandon_current_task() && !executor.abandon_idle_executor() {
        // Already polling but the task could not be retired: still force the
        // runtime to replace this executor rather than resume a dead stack.
        executor.force_replace_executor();
    }
    let executor_cx = executor.context.get_context();
    let runtime_cx = runtime.get_context();
    drop(runtime);
    drop(executor);
    switch(executor_cx, runtime_cx);
    unreachable!("abandoned executor (fault_sp) was resumed");
}

pub unsafe fn abandon_current_executor() -> bool {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CORE_NUM {
        return false;
    }
    let Some(runtime) = GLOBAL_RUNTIME[cpu].try_lock() else {
        return false;
    };
    let Some(executor) = runtime.current_executor.clone() else {
        return false;
    };
    if executor.is_polling() || !executor.stack_contains(current_sp()) {
        return false;
    }
    if !executor.abandon_idle_executor() {
        return false;
    }
    let executor_cx = executor.context.get_context();
    let runtime_cx = runtime.get_context();
    drop(runtime);
    drop(executor);
    switch(executor_cx, runtime_cx);
    unreachable!("abandoned idle executor was resumed");
}

pub unsafe fn abandon_current_task() -> bool {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CORE_NUM {
        return false;
    }
    let Some(runtime) = GLOBAL_RUNTIME[cpu].try_lock() else {
        return false;
    };
    let Some(executor) = runtime.current_executor.clone() else {
        return false;
    };
    if !executor.is_polling()
        || !executor.stack_contains(current_sp())
        || !executor.canary_intact()
    {
        return false;
    }
    if !executor.abandon_current_task() {
        return false;
    }
    // `is_running_future()` is deliberately left true: on the far side of the
    // switch, `run_until_idle` reads it to decide that this executor was
    // interrupted mid-poll and must be replaced by a fresh one rather than
    // resumed. `killed()` (now true) is what keeps it from ever being resumed
    // as a weak executor, and what makes the runtime drop it — freeing the
    // stack we are about to leave.
    let executor_cx = executor.context.get_context();
    let runtime_cx = runtime.get_context();
    drop(runtime);
    drop(executor);
    switch(executor_cx, runtime_cx);
    unreachable!("abandoned executor was resumed");
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
