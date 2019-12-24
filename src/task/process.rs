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
    handles: BTreeMap<HandleValue, Handle>,
    threads: Vec<Arc<Thread>>,
}

impl Process {
    pub fn create(job: &Arc<Job>, name: &str, options: u32) -> ZxResult<Arc<Self>> {
        // TODO: options
        let proc = Arc::new(Process {
            base: KObjectBase::new(),
            name: String::from(name),
            job: job.clone(),
            policy: job.policy(),
            vmar: Arc::new(VmAddressRegion {}),
            inner: Mutex::new(ProcessInner::default()),
        });
        job.add_process(proc.clone());
        Ok(proc)
    }

    pub fn start(
        &mut self,
        thread: &Thread,
        entry: usize,
        stack: usize,
        arg1: Handle,
        arg2: usize,
    ) {
        unimplemented!()
    }

    pub fn exit(&mut self, retcode: usize) {
        unimplemented!()
    }

    /// Check whether `condition` is allowed in the parent job's policy.
    pub fn check_policy(&self, condition: PolicyCondition) -> ZxResult<()> {
        match self.policy.get_action(condition) {
            PolicyAction::Allow => Ok(()),
            PolicyAction::Deny => Err(ZxError::ACCESS_DENIED),
            _ => unimplemented!(),
        }
    }

    /// Add a handle to the process
    pub fn add_handle(&self, handle: Handle) -> HandleValue {
        // FIXME: handle value from ptr
        let mut inner = self.inner.lock();
        let value = (0 as HandleValue..)
            .find(|idx| !inner.handles.contains_key(idx))
            .unwrap();
        inner.handles.insert(value, handle);
        value
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create() {
        let proc = Process::create(&job::ROOT_JOB, "proc", 0).expect("failed to create process");
    }

    #[test]
    fn handle() {
        let proc = Process::create(&job::ROOT_JOB, "proc", 0).expect("failed to create process");
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
