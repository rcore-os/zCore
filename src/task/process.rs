use super::job::Job;
use super::job_policy::JobPolicy;
use super::thread::Thread;
use super::*;
use crate::object::*;
use crate::vm::vmar::VmAddressRegion;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use core::any::Any;

pub struct Process {
    base: KObjectBase,
    name: String,
    job: Arc<Job>,
    policy: JobPolicy,
    vmar: Arc<VmAddressRegion>,
    handles: BTreeMap<HandleValue, Handle>,
}

impl Process {
    pub fn create(job: Arc<Job>, name: &str, options: u32) -> ZxResult<Self> {
        // TODO: options
        // TODO: add proc to job
        let proc = Process {
            base: KObjectBase::new(),
            name: String::from(name),
            policy: job.policy.clone(),
            vmar: Arc::new(VmAddressRegion {}),
            handles: BTreeMap::default(),
            job,
        };
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

    /// Get the kernel object corresponding to this `handle_value`,
    /// after checking that this handle has the `desired_rights`.
    pub fn get_object_with_rights<T: KernelObject>(
        &self,
        handle_value: HandleValue,
        desired_rights: Rights,
    ) -> ZxResult<Arc<dyn KernelObject>> {
        let handle = self.handles.get(&handle_value).ok_or(ZxError::BAD_HANDLE)?;
        // check type before rights
        if handle.object.downcast_ref::<T>().is_none() {
            return Err(ZxError::WRONG_TYPE);
        }
        if !handle.rights.contains(desired_rights) {
            return Err(ZxError::ACCESS_DENIED);
        }
        Ok(handle.object.clone())
    }
}
