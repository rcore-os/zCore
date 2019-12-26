use super::job::Job;
use super::job_policy::*;
use super::thread::Thread;
use super::*;
use crate::object::*;
use crate::vm::vmar::VmAddressRegion;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use spin::Mutex;

pub struct Process {
    base: KObjectBase,
    name: String,
    job: Arc<Job>,
    policy: JobPolicy,
    vmar: Arc<VmAddressRegion>,
    inner: Mutex<ProcessInner>,
}

impl_kobject!(Process);

#[derive(Default)]
struct ProcessInner {
    started: bool,
    handles: BTreeMap<HandleValue, Handle>,
    threads: Vec<Arc<Thread>>,
}

impl Process {
    /// Create a new process in the `job`.
    pub fn create(job: &Arc<Job>, name: &str, options: u32) -> ZxResult<Arc<Self>> {
        // TODO: options
        let proc = Arc::new(Process {
            base: KObjectBase::new(),
            name: String::from(name),
            job: job.clone(),
            policy: job.policy(),
            vmar: VmAddressRegion::new_root(),
            inner: Mutex::new(ProcessInner::default()),
        });
        job.add_process(proc.clone());
        Ok(proc)
    }

    /// Start the first `thread` in the process.
    ///
    /// This causes a thread to begin execution at the program
    /// counter specified by `entry` and with the stack pointer set to `stack`.
    /// The arguments `arg1` and `arg2` are arranged to be in the architecture
    /// specific registers used for the first two arguments of a function call
    /// before the thread is started. All other registers are zero upon start.
    pub fn start(
        &self,
        thread: &Arc<Thread>,
        entry: usize,
        stack: usize,
        arg1: Handle,
        arg2: usize,
    ) -> ZxResult<()> {
        let mut inner = self.inner.lock();
        if !inner.contains_thread(thread) {
            return Err(ZxError::ACCESS_DENIED);
        }
        let handle_value = inner.add_handle(arg1);
        if inner.started {
            return Err(ZxError::BAD_STATE);
        }
        inner.started = true;
        thread.start(entry, stack, handle_value as usize, arg2)?;
        Ok(())
    }

    pub fn exit(&self, retcode: usize) {
        unimplemented!()
    }

    /// Check whether `condition` is allowed in the parent job's policy.
    pub fn check_policy(&self, condition: PolicyCondition) -> ZxResult<()> {
        match self
            .policy
            .get_action(condition)
            .unwrap_or(PolicyAction::Allow)
        {
            PolicyAction::Allow => Ok(()),
            PolicyAction::Deny => Err(ZxError::ACCESS_DENIED),
            _ => unimplemented!(),
        }
    }

    /// Add a handle to the process
    pub fn add_handle(&self, handle: Handle) -> HandleValue {
        self.inner.lock().add_handle(handle)
    }

    /// Remove a handle from the process
    pub fn remove_handle(&self, handle_value: HandleValue) {
        self.inner.lock().handles.remove(&handle_value);
    }

    /// Get the kernel object corresponding to this `handle_value`,
    /// after checking that this handle has the `desired_rights`.
    pub fn get_object_with_rights<T: KernelObject>(
        &self,
        handle_value: HandleValue,
        desired_rights: Rights,
    ) -> ZxResult<Arc<T>> {
        let handle = self
            .inner
            .lock()
            .handles
            .get(&handle_value)
            .ok_or(ZxError::BAD_HANDLE)?
            .clone();
        // check type before rights
        let object = handle
            .object
            .downcast_arc::<T>()
            .map_err(|_| ZxError::WRONG_TYPE)?;
        if !handle.rights.contains(desired_rights) {
            return Err(ZxError::ACCESS_DENIED);
        }
        Ok(object)
    }

    /// Add a thread to the process.
    pub(super) fn add_thread(&self, thread: Arc<Thread>) {
        self.inner.lock().threads.push(thread);
    }
}

impl ProcessInner {
    /// Add a handle to the process
    fn add_handle(&mut self, handle: Handle) -> HandleValue {
        // FIXME: handle value from ptr
        let value = (0 as HandleValue..)
            .find(|idx| !self.handles.contains_key(idx))
            .unwrap();
        self.handles.insert(value, handle);
        value
    }

    /// Whether `thread` is in this process.
    fn contains_thread(&self, thread: &Arc<Thread>) -> bool {
        self.threads
            .iter()
            .find(|&t| Arc::ptr_eq(t, thread))
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc", 0).expect("failed to create process");
    }

    #[test]
    fn handle() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc", 0).expect("failed to create process");
        let handle = Handle::new(proc.clone(), Rights::DEFAULT_PROCESS);

        let handle_value = proc.add_handle(handle);

        // getting object should success
        let object: Arc<Process> = proc
            .get_object_with_rights(handle_value, Rights::DEFAULT_PROCESS)
            .expect("failed to get object");
        assert!(Arc::ptr_eq(&object, &proc));

        // getting object with an extra rights should fail.
        assert_eq!(
            proc.get_object_with_rights::<Process>(handle_value, Rights::MANAGE_JOB)
                .err(),
            Some(ZxError::ACCESS_DENIED)
        );

        // getting object with invalid type should fail.
        assert_eq!(
            proc.get_object_with_rights::<Job>(handle_value, Rights::DEFAULT_PROCESS)
                .err(),
            Some(ZxError::WRONG_TYPE)
        );

        proc.remove_handle(handle_value);

        // getting object with invalid handle should fail.
        assert_eq!(
            proc.get_object_with_rights::<Process>(handle_value, Rights::DEFAULT_PROCESS)
                .err(),
            Some(ZxError::BAD_HANDLE)
        );
    }
}
