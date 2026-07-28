use crate::{executor::Executor, task_collection::*, waker_page::WakerRef};

#[cfg(target_arch = "x86_64")]
use crate::context::Context;
#[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
use crate::context::ContextData as Context;

use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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

/// Waker-side: kick `owner` with the wake IPI if it is (about to be) halted.
/// The caller must have already published the wake (SeqCst) — see the pairing
/// argument in `WakerRef::wake_by_ref`.
#[inline]
pub(crate) fn maybe_send_resched_ipi(owner: u8) {
    let owner = owner as usize;
    if owner >= 64 || SLEEPING_CPUS.load(Ordering::SeqCst) & (1 << owner) == 0 {
        return;
    }
    let f = RESCHED_IPI_SENDER.load(Ordering::Acquire);
    if f != 0 {
        let f: fn(usize) = unsafe { core::mem::transmute(f) };
        f(owner);
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
            let count = runtime.task_num();
            if count > 0 {
                candidates[n] = (i, count);
                n += 1;
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
        // fewest tasks wins. Only CPUs allowed by the affinity mask are
        // considered, so an affine task is born on a legal CPU instead of being
        // placed anywhere and then bounced off by the affinity check.
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
                let count = rt.task_num();
                if count < best_count {
                    best_count = count;
                    best = i;
                }
            }
        }
        (&GLOBAL_RUNTIME[best], best)
    };
    crate::diag::diag_lock(runtime).add_task(priority, future, affinity);
    // A new task is born `notified` on `target_cpu`'s queue; if that CPU is
    // halted it would not notice until its next tick. Same kick as a wake.
    maybe_send_resched_ipi(target_cpu as u8);
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
