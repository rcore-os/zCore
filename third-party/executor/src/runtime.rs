use crate::{
    executor::Executor,
    task_collection::*,
    waker_page::{DroperRef, WakerRef},
};

#[cfg(target_arch = "x86_64")]
use crate::context::Context;
#[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
use crate::context::ContextData as Context;

use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
use core::{future::Future, pin::Pin};
use lazy_static::*;
use spin::{Mutex, MutexGuard};

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
    fn add_task<F: Future<Output = ()> + 'static + Send>(&self, priority: usize, future: F) -> Key {
        debug_assert!(priority < MAX_PRIORITY);
        self.task_collection.add_task(future)
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
    pub static ref GLOBAL_RUNTIME: [Mutex<ExecutorRuntime>; 5] = [
        Mutex::new(ExecutorRuntime::new(0)),
        Mutex::new(ExecutorRuntime::new(1)),
        Mutex::new(ExecutorRuntime::new(2)),
        Mutex::new(ExecutorRuntime::new(3)),
        Mutex::new(ExecutorRuntime::new(4))
    ];
}

// obtain a task from other cpu.
pub(crate) fn steal_task_from_other_cpu() -> Option<(Key, Arc<Task>, WakerRef, DroperRef)> {
    let runtime = GLOBAL_RUNTIME
        .iter()
        .max_by_key(|runtime| runtime.lock().task_num())
        .unwrap();
    let runtime = runtime.lock();
    if runtime.task_num() > 0 {
        runtime.task_collection.take_task()
    } else {
        None
    }
}

// per-cpu scheduler.
pub fn run_until_idle() -> bool {
    debug!("GLOBAL_RUNTIME.run()");
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
        spawn_task(future, None, Some(crate::arch::cpu_id() as _))
    }
}

/// Spawn a coroutine with `priority` and `cpu_id`
/// Default priority: DEFAULT_PRIORITY
/// Default cpu_id: the cpu with fewest number of tasks
pub fn spawn_task(
    future: impl Future<Output = ()> + Send + 'static,
    priority: Option<usize>,
    cpu_id: Option<usize>,
) {
    debug!("try to spawn {:?} {:?}", priority, cpu_id);
    let priority = priority.unwrap_or(DEFAULT_PRIORITY);
    let runtime = if let Some(cpu_id) = cpu_id {
        &GLOBAL_RUNTIME[cpu_id]
    } else {
        GLOBAL_RUNTIME
            .iter()
            .min_by_key(|runtime| runtime.lock().task_num())
            .unwrap()
    };
    runtime.lock().add_task(priority, future);
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
    GLOBAL_RUNTIME[crate::arch::cpu_id() as usize].lock()
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
