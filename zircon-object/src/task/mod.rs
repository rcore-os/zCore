//! Objects for Task Management.

use {super::*, crate::ipc::Channel, crate::signal::Port};

mod exception;
mod job;
mod job_policy;
mod process;
mod suspend_token;
mod thread;

pub use {
    self::job::*, self::job_policy::*, self::process::*, self::suspend_token::*, self::thread::*,
};

/// Task (Thread, Process, or Job)
pub trait Task: Sync + Send {
    /// Kill the task.
    fn kill(&self);

    /// Suspend the task. Currently only thread or process handles may be suspended.
    fn suspend(&self);

    /// Resume the task
    fn resume(&self);

    /// Create an exception channel on the task.
    fn create_exception_channel(&mut self, options: u32) -> ZxResult<Channel>;

    /// Resume the task from a previously caught exception.
    fn resume_from_exception(&mut self, port: &Port, options: u32) -> ZxResult;
}
