//! https://github.com/rcore-os/executor/blob/master/executor/src/lib.rs

use {
    alloc::{boxed::Box, collections::vec_deque::VecDeque, sync::Arc},
    core::{
        future::Future,
        pin::Pin,
        task::{Context, Poll},
    },
    woke::{waker_ref, Woke},
};
use crate::futex::{FMutex, FSem};
use lazy_static::lazy_static;

static WAKE_SEM: FSem = FSem::new(0);

/// Executor holds a list of tasks to be processed
#[derive(Default)]
pub struct Executor {
    tasks: VecDeque<Arc<Task>>,
}

/// Task is our unit of execution and holds a future are waiting on
pub struct Task {
    pub future: FMutex<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    state: FMutex<bool>,
}

/// Implement what we would like to do when a task gets woken up
impl Woke for Task {
    fn wake_by_ref(task: &Arc<Self>) {
        task.mark_ready();
        WAKE_SEM.up();
    }
}

impl Task {
    fn mark_ready(&self) {
        let mut value = self.state.lock();
        *value = true;
    }

    pub fn is_sleeping(&self) -> bool {
        let value = self.state.lock();
        !(*value)
    }

    pub fn mark_sleep(&self) {
        let mut value = self.state.lock();
        *value = false;
    }
}

impl Executor {
    /// Add task for a future to the list of tasks
    fn add_task(&mut self, future: Pin<Box<dyn Future<Output = ()> + 'static + Send>>) {
        // store our task
        let task = Arc::new(Task {
            future: FMutex::new(future),
            state: FMutex::new(true),
        });
        self.tasks.push_back(task);
    }

    pub fn push_task(&mut self, task: Arc<Task>) {
        self.tasks.push_back(task);
    }

    pub fn pop_runnable_task(&mut self) -> Option<Arc<Task>> {
        for _ in 0..self.tasks.len() {
            let task = self.tasks.pop_front().unwrap();
            if !task.is_sleeping() {
                return Some(task);
            }
            self.tasks.push_back(task);
        }
        None
    }
}

lazy_static! {
    static ref GLOBAL_EXECUTOR: FMutex<Executor> = FMutex::new(Executor::default());
}

/// Give future to global executor to be polled and executed.
pub fn spawn(future: impl Future<Output = ()> + 'static + Send) {
    GLOBAL_EXECUTOR.lock().add_task(Box::pin(future));
    WAKE_SEM.up();
}

/// Run futures until there is no runnable task.
pub fn run_until_idle() {
    while let Some(task) = { || GLOBAL_EXECUTOR.lock().pop_runnable_task() }() {
        task.mark_sleep();
        // make a waker for our task
        let waker = waker_ref(&task);
        // poll our future and give it a waker
        let mut context = Context::from_waker(&*waker);
        let ret = task.future.lock().as_mut().poll(&mut context);
        if let Poll::Pending = ret {
            GLOBAL_EXECUTOR.lock().push_task(task);
        }
    }
}

pub fn run() -> ! {
    loop {
        run_until_idle();
        WAKE_SEM.down();
    }
}

pub fn init() {
    use crate::kt;
    for _ in 0..16 {
        kt::spawn(|| run()).expect("cannot spawn async executor");
    }
}