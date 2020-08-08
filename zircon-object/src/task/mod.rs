#![deny(missing_docs)]
//! Objects for Task Management.

use super::*;

mod exception;
mod job;
mod job_policy;
mod process;
mod suspend_token;
mod thread;

pub use {
    self::exception::*, self::job::*, self::job_policy::*, self::process::*,
    self::suspend_token::*, self::thread::*,
};

/// Task (Thread, Process, or Job)
pub trait Task: Sync + Send {
    /// Kill the task.
    fn kill(&self);

    /// Suspend the task. Currently only thread or process handles may be suspended.
    fn suspend(&self);

    /// Resume the task
    fn resume(&self);
}

/// The return code set when a task is killed via zx_task_kill().
pub const TASK_RETCODE_SYSCALL_KILL: i64 = -1024;
