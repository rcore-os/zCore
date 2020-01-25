use {
    super::{job::Job, job_policy::*, resource::*, thread::Thread, *},
    crate::{object::*, vm::*},
    alloc::{boxed::Box, collections::BTreeMap, string::String, sync::Arc, vec::Vec},
    core::any::Any,
    spin::Mutex,
};

/// Process abstraction
///
/// ## SYNOPSIS
///
/// A zircon process is an instance of a program in the traditional
/// sense: a set of instructions which will be executed by one or more
/// threads, along with a collection of resources.
///
/// ## DESCRIPTION
///
/// The process object is a container of the following resources:
///
/// + [Handles](crate::object::Handle)
/// + [Virtual Memory Address Regions](crate::vm::VmAddressRegion)
/// + [Threads](crate::task::Thread)
///
/// In general, it is associated with code which it is executing until it is
/// forcefully terminated or the program exits.
///
/// Processes are owned by [jobs](job.html) and allow an application that is
/// composed by more than one process to be treated as a single entity, from the
/// perspective of resource and permission limits, as well as lifetime control.
///
/// ### Lifetime
/// A process is created via [`Process::create()`] and its execution begins with
/// [`Process::start()`].
///
/// The process stops execution when:
/// - the last thread is terminated or exits
/// - the process calls [`Process::exit()`]
/// - the parent job terminates the process
/// - the parent job is destroyed
///
/// The call to [`Process::start()`] cannot be issued twice. New threads cannot
/// be added to a process that was started and then its last thread has exited.
///
/// [`Process::create()`]: Process::create
/// [`Process::start()`]: Process::start
/// [`Process::exit()`]: Process::exit
#[allow(dead_code)]
pub struct Process {
    base: KObjectBase,
    name: String,
    job: Arc<Job>,
    policy: JobPolicy,
    vmar: Arc<VmAddressRegion>,
    ext: Box<dyn Any + Send + Sync>,
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
    pub fn create(job: &Arc<Job>, name: &str, _options: u32) -> ZxResult<Arc<Self>> {
        Self::create_with_ext(job, name, ())
    }

    /// Create a new process with extension info.
    pub fn create_with_ext(
        job: &Arc<Job>,
        name: &str,
        ext: impl Any + Send + Sync,
    ) -> ZxResult<Arc<Self>> {
        // TODO: _options -> options
        let proc = Arc::new(Process {
            base: KObjectBase::new(),
            name: String::from(name),
            job: job.clone(),
            policy: job.policy(),
            vmar: VmAddressRegion::new_root(),
            ext: Box::new(ext),
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

    pub fn exit(&self, _retcode: i64) {
        // TODO: exit process
        self.base.signal_set(Signal::PROCESS_TERMINATED);
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

    /// Get the extension.
    pub fn ext(&self) -> &Box<dyn Any + Send + Sync> {
        &self.ext
    }

    /// Get the `VmAddressRegion` of the process.
    pub fn vmar(&self) -> Arc<VmAddressRegion> {
        self.vmar.clone()
    }

    /// Add a handle to the process
    pub fn add_handle(&self, handle: Handle) -> HandleValue {
        self.inner.lock().add_handle(handle)
    }

    /// Remove a handle from the process
    pub fn remove_handle(&self, handle_value: HandleValue) -> ZxResult<()> {
        match self.inner.lock().handles.remove(&handle_value) {
            Some(_) => Ok(()),
            None => Err(ZxError::BAD_HANDLE),
        }
    }

    /// Get a handle from the process
    fn get_handle(&self, handle_value: HandleValue) -> ZxResult<Handle> {
        self.inner
            .lock()
            .handles
            .get(&handle_value)
            .cloned()
            .ok_or(ZxError::BAD_HANDLE)
    }

    /// Duplicate a handle with new `rights`, return the new handle value.
    ///
    /// The handle must have `Rights::DUPLICATE`.
    /// To duplicate the handle with the same rights use `Rights::SAME_RIGHTS`.
    /// If different rights are desired they must be strictly lesser than of the source handle,
    /// or an `ZxError::ACCESS_DENIED` will be raised.
    pub fn dup_handle(&self, handle_value: HandleValue, rights: Rights) -> ZxResult<HandleValue> {
        let mut inner = self.inner.lock();
        let mut handle = match inner.handles.get(&handle_value) {
            Some(h) => h.clone(),
            None => return Err(ZxError::BAD_HANDLE),
        };
        if !handle.rights.contains(Rights::DUPLICATE) {
            return Err(ZxError::ACCESS_DENIED);
        }
        if !rights.contains(Rights::SAME_RIGHTS) {
            // `rights` must be strictly lesser than of the source handle
            if !(handle.rights.contains(rights) && handle.rights != rights) {
                return Err(ZxError::INVALID_ARGS);
            }
            handle.rights = rights;
        }
        let new_handle_value = inner.add_handle(handle);
        Ok(new_handle_value)
    }

    /// Get the kernel object corresponding to this `handle_value`,
    /// after checking that this handle has the `desired_rights`.
    pub fn get_object_with_rights<T: KernelObject>(
        &self,
        handle_value: HandleValue,
        desired_rights: Rights,
    ) -> ZxResult<Arc<T>> {
        let handle = self.get_handle(handle_value)?;
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

    /// Get the kernel object corresponding to this `handle_value`
    pub fn get_object<T: KernelObject>(&self, handle_value: HandleValue) -> ZxResult<Arc<T>> {
        let handle = self.get_handle(handle_value)?;
        let object = handle
            .object
            .downcast_arc::<T>()
            .map_err(|_| ZxError::WRONG_TYPE)?;
        Ok(object)
    }

    /// Try to get Resource and validate it
    pub fn validate_resource(&self, handle_value: HandleValue, kind: ResourceKind) -> ZxResult<()> {
        let handle = self.get_handle(handle_value)?;
        let object = handle
            .object
            .downcast_arc::<Resource>()
            .map_err(|_| ZxError::WRONG_TYPE)?;
        object.validate(kind)
    }

    /// Equal to `get_object_with_rights<dyn VMObject>`.
    pub fn get_vmo_with_rights(
        &self,
        handle_value: HandleValue,
        desired_rights: Rights,
    ) -> ZxResult<Arc<dyn VMObject>> {
        let handle = self.get_handle(handle_value)?;
        // check type before rights
        let object: Arc<dyn VMObject> = handle
            .object
            .downcast_arc::<VMObjectPaged>()
            .map(|obj| obj as Arc<dyn VMObject>)
            .or_else(|obj| {
                obj.downcast_arc::<VMObjectPhysical>()
                    .map(|obj| obj as Arc<dyn VMObject>)
            })
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
        info!("A new handle is added : {}", value);
        value
    }

    /// Whether `thread` is in this process.
    fn contains_thread(&self, thread: &Arc<Thread>) -> bool {
        self.threads.iter().any(|t| Arc::ptr_eq(t, thread))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create() {
        let root_job = Job::root();
        let _proc = Process::create(&root_job, "proc", 0).expect("failed to create process");
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

        proc.remove_handle(handle_value).unwrap();

        // getting object with invalid handle should fail.
        assert_eq!(
            proc.get_object_with_rights::<Process>(handle_value, Rights::DEFAULT_PROCESS)
                .err(),
            Some(ZxError::BAD_HANDLE)
        );
    }

    #[test]
    fn handle_duplicate() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc", 0).expect("failed to create process");

        // duplicate non-exist handle should fail.
        assert_eq!(
            proc.dup_handle(0, Rights::empty()),
            Err(ZxError::BAD_HANDLE)
        );

        // duplicate handle with the same rights.
        let rights = Rights::DUPLICATE;
        let handle_value = proc.add_handle(Handle::new(proc.clone(), rights));
        let new_handle_value = proc.dup_handle(handle_value, Rights::SAME_RIGHTS).unwrap();
        assert_eq!(proc.get_handle(new_handle_value).unwrap().rights, rights);

        // duplicate handle with subset rights.
        let new_handle_value = proc.dup_handle(handle_value, Rights::empty()).unwrap();
        assert_eq!(
            proc.get_handle(new_handle_value).unwrap().rights,
            Rights::empty()
        );

        // duplicate handle with more rights should fail.
        assert_eq!(
            proc.dup_handle(handle_value, Rights::READ),
            Err(ZxError::INVALID_ARGS)
        );

        // duplicate handle which does not have `Rights::DUPLICATE` should fail.
        let handle_value = proc.add_handle(Handle::new(proc.clone(), Rights::empty()));
        assert_eq!(
            proc.dup_handle(handle_value, Rights::SAME_RIGHTS),
            Err(ZxError::ACCESS_DENIED)
        );
    }
}
