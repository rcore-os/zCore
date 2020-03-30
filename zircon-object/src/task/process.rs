use {
    super::{job::Job, job_policy::*, resource::*, thread::Thread, *},
    crate::{object::*, signal::Futex, vm::*},
    alloc::{boxed::Box, collections::BTreeMap, sync::Arc, vec::Vec},
    core::{any::Any, sync::atomic::AtomicI32},
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
    job: Arc<Job>,
    policy: JobPolicy,
    vmar: Arc<VmAddressRegion>,
    ext: Box<dyn Any + Send + Sync>,
    inner: Mutex<ProcessInner>,
}

impl_kobject!(Process
    fn get_child(&self, id: KoID) -> ZxResult<Arc<dyn KernelObject>> {
        let inner = self.inner.lock();
        let thread = inner.threads.iter().find(|o| o.id() == id).ok_or(ZxError::NOT_FOUND)?;
        Ok(thread.clone())
    }
    fn related_koid(&self) -> KoID {
        self.job.id()
    }
);

#[derive(Default)]
struct ProcessInner {
    status: Status,
    max_handle_id: u32,
    handles: BTreeMap<HandleValue, Handle>,
    futexes: BTreeMap<usize, Arc<Futex>>,
    threads: Vec<Arc<Thread>>,

    // special info
    debug_addr: usize,
    dyn_break_on_load: usize,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Status {
    Init,
    Running,
    Exited(i64),
}

impl Default for Status {
    fn default() -> Self {
        Status::Init
    }
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
            base: KObjectBase::with_name(name),
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
    ) -> ZxResult {
        let handle_value;
        {
            let mut inner = self.inner.lock();
            if !inner.contains_thread(thread) {
                return Err(ZxError::ACCESS_DENIED);
            }
            handle_value = inner.add_handle(arg1);
            if inner.status != Status::Init {
                return Err(ZxError::BAD_STATE);
            }
            inner.status = Status::Running;
        }
        thread.start(entry, stack, handle_value as usize, arg2)?;
        Ok(())
    }

    /// Exit current process with `retcode`.
    pub fn exit(&self, retcode: i64) {
        let mut inner = self.inner.lock();
        inner.status = Status::Exited(retcode);
        // TODO: exit all threads
        self.base.signal_set(Signal::PROCESS_TERMINATED);
        for thread in inner.threads.iter() {
            thread.internal_exit();
        }
        inner.threads.clear();
        inner.handles.clear();
        self.job.process_exit(self.base.id, retcode);
    }

    /// Check whether `condition` is allowed in the parent job's policy.
    pub fn check_policy(&self, condition: PolicyCondition) -> ZxResult {
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

    /// Get process status.
    pub fn status(&self) -> Status {
        self.inner.lock().status
    }

    /// Get the extension.
    pub fn ext(&self) -> &Box<dyn Any + Send + Sync> {
        &self.ext
    }

    /// Get the `VmAddressRegion` of the process.
    pub fn vmar(&self) -> Arc<VmAddressRegion> {
        self.vmar.clone()
    }

    /// Get the job of the process.
    pub fn job(&self) -> Arc<Job> {
        self.job.clone()
    }

    /// Add a handle to the process
    pub fn add_handle(&self, handle: Handle) -> HandleValue {
        self.inner.lock().add_handle(handle)
    }

    /// Add all handles to the process
    pub fn add_handles(&self, handles: Vec<Handle>) -> Vec<HandleValue> {
        let mut inner = self.inner.lock();
        handles.into_iter().map(|h| inner.add_handle(h)).collect()
    }

    /// Remove a handle from the process
    pub fn remove_handle(&self, handle_value: HandleValue) -> ZxResult<Handle> {
        self.inner
            .lock()
            .handles
            .remove(&handle_value)
            .ok_or(ZxError::BAD_HANDLE)
    }

    /// Remove all handles from the process.
    ///
    /// If one or more error happens, return one of them.
    /// All handles are discarded on success or failure.
    pub fn remove_handles(&self, handle_values: &[HandleValue]) -> ZxResult<Vec<Handle>> {
        let mut inner = self.inner.lock();
        handle_values
            .iter()
            .map(|h| inner.handles.remove(h).ok_or(ZxError::BAD_HANDLE))
            .collect()
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

    /// Get a futex from the process
    pub fn get_futex(&self, addr: &'static AtomicI32) -> Arc<Futex> {
        let mut inner = self.inner.lock();
        inner
            .futexes
            .entry(addr as *const AtomicI32 as usize)
            .or_insert_with(|| Futex::new(addr))
            .clone()
    }

    /// Duplicate a handle with new `rights`, return the new handle value.
    ///
    /// The handle must have `Rights::DUPLICATE`.
    /// To duplicate the handle with the same rights use `Rights::SAME_RIGHTS`.
    /// If different rights are desired they must be strictly lesser than of the source handle,
    /// or an `ZxError::ACCESS_DENIED` will be raised.
    pub fn dup_handle_operating_rights(
        &self,
        handle_value: HandleValue,
        operation: impl FnOnce(Rights) -> ZxResult<Rights>,
    ) -> ZxResult<HandleValue> {
        let mut inner = self.inner.lock();
        let mut handle = match inner.handles.get(&handle_value) {
            Some(h) => h.clone(),
            None => return Err(ZxError::BAD_HANDLE),
        };
        handle.rights = operation(handle.rights)?;
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

    pub fn get_object_and_rights<T: KernelObject>(
        &self,
        handle_value: HandleValue,
    ) -> ZxResult<(Arc<T>, Rights)> {
        let handle = self.get_handle(handle_value)?;
        // check type before rights
        let object = handle
            .object
            .downcast_arc::<T>()
            .map_err(|_| ZxError::WRONG_TYPE)?;
        Ok((object, handle.rights))
    }

    pub fn get_dyn_object_with_rights(
        &self,
        handle_value: HandleValue,
        desired_rights: Rights,
    ) -> ZxResult<Arc<dyn KernelObject>> {
        let handle = self.get_handle(handle_value)?;
        // check type before rights
        if !handle.rights.contains(desired_rights) {
            return Err(ZxError::ACCESS_DENIED);
        }
        Ok(handle.object)
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
    pub fn validate_resource(&self, handle_value: HandleValue, kind: ResourceKind) -> ZxResult {
        let handle = self.get_handle(handle_value)?;
        let object = handle
            .object
            .downcast_arc::<Resource>()
            .map_err(|_| ZxError::WRONG_TYPE)?;
        object.validate(kind)
    }

    pub fn get_handle_info(&self, handle_value: HandleValue) -> ZxResult<HandleBasicInfo> {
        let handle = self.get_handle(handle_value)?;
        Ok(handle.get_info())
    }

    /// Add a thread to the process.
    pub(super) fn add_thread(&self, thread: Arc<Thread>) {
        let mut inner = self.inner.lock();
        if let Status::Exited(_) = inner.status {
            panic!("can not add thread to exited process");
        }
        inner.threads.push(thread);
    }

    /// Remove a thread to from process.
    ///
    /// If no more threads left, exit the process.
    pub(super) fn remove_thread(&self, tid: KoID) {
        let mut inner = self.inner.lock();
        let idx = inner.threads.iter().position(|t| t.id() == tid).unwrap();
        inner.threads.remove(idx);
        if inner.threads.is_empty() {
            drop(inner);
            self.exit(0);
        }
    }

    pub fn get_info(&self) -> ProcessInfo {
        let mut info = ProcessInfo::default();
        // TODO correct debugger_attached setting
        info.debugger_attached = false;
        match self.inner.lock().status {
            Status::Init => {
                info.started = false;
                info.has_exited = false;
            }
            Status::Running => {
                info.started = true;
                info.has_exited = false;
            }
            Status::Exited(ret) => {
                info.return_code = ret;
                info.has_exited = true;
                info.started = false;
            }
        }
        info
    }

    pub fn set_debug_addr(&self, addr: usize) {
        self.inner.lock().debug_addr = addr;
    }

    pub fn get_debug_addr(&self) -> usize {
        self.inner.lock().debug_addr
    }

    pub fn set_dyn_break_on_load(&self, addr: usize) {
        self.inner.lock().dyn_break_on_load = addr;
    }

    pub fn get_dyn_break_on_load(&self) -> usize {
        self.inner.lock().dyn_break_on_load
    }
}

impl ProcessInner {
    /// Add a handle to the process
    fn add_handle(&mut self, handle: Handle) -> HandleValue {
        // FIXME: handle value from ptr
        let key = (self.max_handle_id << 2) | 0x3u32;
        info!("add handle: {:#x}, {:?}", key, handle.object);
        self.max_handle_id += 1;
        self.handles.insert(key, handle);
        key
    }

    /// Whether `thread` is in this process.
    fn contains_thread(&self, thread: &Arc<Thread>) -> bool {
        self.threads.iter().any(|t| Arc::ptr_eq(t, thread))
    }
}

#[repr(C)]
#[derive(Default)]
pub struct ProcessInfo {
    pub return_code: i64,
    pub started: bool,
    pub has_exited: bool,
    pub debugger_attached: bool,
    pub padding1: [u8; 5],
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
            proc.dup_handle_operating_rights(0, |_| Ok(Rights::empty())),
            Err(ZxError::BAD_HANDLE)
        );

        // duplicate handle with the same rights.
        let rights = Rights::DUPLICATE;
        let handle_value = proc.add_handle(Handle::new(proc.clone(), rights));
        let new_handle_value = proc
            .dup_handle_operating_rights(handle_value, |old_rights| Ok(old_rights))
            .unwrap();
        assert_eq!(proc.get_handle(new_handle_value).unwrap().rights, rights);

        // duplicate handle with subset rights.
        let new_handle_value = proc
            .dup_handle_operating_rights(handle_value, |_| Ok(Rights::empty()))
            .unwrap();
        assert_eq!(
            proc.get_handle(new_handle_value).unwrap().rights,
            Rights::empty()
        );

        // duplicate handle which does not have `Rights::DUPLICATE` should fail.
        let handle_value = proc.add_handle(Handle::new(proc.clone(), Rights::empty()));
        assert_eq!(
            proc.dup_handle_operating_rights(handle_value, |handle_rights| {
                if !handle_rights.contains(Rights::DUPLICATE) {
                    return Err(ZxError::ACCESS_DENIED);
                }
                Ok(handle_rights)
            }),
            Err(ZxError::ACCESS_DENIED)
        );
    }

    #[test]
    fn get_child() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc", 0).expect("failed to create process");
        let thread = Thread::create(&proc, "thread", 0).expect("failed to create thread");

        let proc: Arc<dyn KernelObject> = proc;
        assert_eq!(proc.get_child(thread.id()).unwrap().id(), thread.id());
        assert_eq!(proc.get_child(proc.id()).err(), Some(ZxError::NOT_FOUND));
    }
}
