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
use crate::task_collection::TaskCollection;

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

const STACK_SIZE: usize = 4096 * 32;
const STACK_LAYOUT: Layout = Layout::new::<[u8; STACK_SIZE]>();

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
        loop {
            let mut task_info = self.task_collection.take_task();
            if task_info.is_none() {
                task_info = crate::runtime::steal_task_from_other_cpu();
            }
            if let Some((_key, task, waker_ref, droper)) = task_info {
                let waker_ref = Arc::new(waker_ref);
                let waker = woke::waker_ref(&waker_ref);
                let mut cx = Context::from_waker(&waker);
                waker_ref.mark_borrowed(true);
                self.task_id = task.id();
                debug!("running future {}:{}", self.id(), task.id());
                let ret = task.poll(&mut cx);
                debug!("back from future {}:{}", self.id(), task.id());
                self.task_id = 0;
                waker_ref.mark_borrowed(false);
                match ret {
                    Poll::Ready(()) => {
                        debug!("task over id = {}", task.id());
                        droper.drop_by_ref();
                    }
                    Poll::Pending => {
                        // Do Nothing
                    }
                };
                if let ExecutorState::WEAK = self.state {
                    self.state = ExecutorState::KILLED;
                    return;
                }
            } else {
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
                    crate::runtime::sched_yield();
                } else {
                    debug!("no other tasks, wait for interrupt");
                    crate::arch::wait_for_interrupt();
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
