use {
    super::{exception::*, job::Job, job_policy::*, thread::Thread, *},
    crate::{object::*, signal::Futex, vm::*},
    alloc::{boxed::Box, sync::Arc, vec::Vec},
    core::{any::Any, sync::atomic::AtomicI32},
    futures::channel::oneshot::{self, Receiver, Sender},
    hashbrown::HashMap,
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
    _counter: CountHelper,
    job: Arc<Job>,
    policy: JobPolicy,
    vmar: Arc<VmAddressRegion>,
    ext: Box<dyn Any + Send + Sync>,
    exceptionate: Arc<Exceptionate>,
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
define_count_helper!(Process);

#[derive(Default)]
struct ProcessInner {
    status: Status,
    max_handle_id: u32,
    handles: HashMap<HandleValue, (Handle, Vec<Sender<()>>)>,
    futexes: HashMap<usize, Arc<Futex>>,
    threads: Vec<Arc<Thread>>,

    // special info
    debug_addr: usize,
    dyn_break_on_load: usize,
    critical_to_job: Option<(Arc<Job>, bool)>,
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
            _counter: CountHelper::new(),
            job: job.clone(),
            policy: job.policy(),
            vmar: VmAddressRegion::new_root(),
            ext: Box::new(ext),
            exceptionate: Exceptionate::new(ExceptionChannelType::Process),
            inner: Mutex::new(ProcessInner::default()),
        });
        job.add_process(proc.clone())?;
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
        arg1: Option<Handle>,
        arg2: usize,
        spawn_fn: fn(thread: Arc<Thread>),
    ) -> ZxResult {
        let handle_value;
        {
            let mut inner = self.inner.lock();
            if !inner.contains_thread(thread) {
                return Err(ZxError::ACCESS_DENIED);
            }
            if inner.status != Status::Init {
                return Err(ZxError::BAD_STATE);
            }
            inner.status = Status::Running;
            handle_value = arg1.map_or(INVALID_HANDLE, |handle| inner.add_handle(handle));
        }
        match thread.start(entry, stack, handle_value as usize, arg2, spawn_fn) {
            Ok(_) => Ok(()),
            Err(err) => {
                let mut inner = self.inner.lock();
                if handle_value != INVALID_HANDLE {
                    inner.remove_handle(handle_value).ok();
                }
                Err(err)
            }
        }
    }

    /// Exit current process with `retcode`.
    pub fn exit(&self, retcode: i64) {
        let mut inner = self.inner.lock();
        inner.status = Status::Exited(retcode);
        // TODO: exit all threads
        self.base.signal_set(Signal::PROCESS_TERMINATED);
        for thread in inner.threads.iter() {
            thread.kill();
        }
        inner.threads.clear();
        inner.handles.clear();

        self.job.process_exit(self.base.id);
        // If we are critical to a job, we need to take action.
        if let Some((_job, retcode_nonzero)) = &inner.critical_to_job {
            if !retcode_nonzero || retcode != 0 {
                unimplemented!("kill the job")
            }
        }
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

    /// Set a process as critical to the job.
    ///
    /// When process terminates, job will be terminated as if `task_kill()` was
    /// called on it. The return code used will be `ZX_TASK_RETCODE_CRITICAL_PROCESS_KILL`.
    ///
    /// The job specified must be the parent of process, or an ancestor.
    ///
    /// If `retcode_nonzero` is true, then job will only be terminated if process
    /// has a non-zero return code.
    pub fn set_critical_at_job(
        &self,
        critical_to_job: &Arc<Job>,
        retcode_nonzero: bool,
    ) -> ZxResult {
        let mut inner = self.inner.lock();
        if inner.critical_to_job.is_some() {
            return Err(ZxError::ALREADY_BOUND);
        }

        let mut job = self.job.clone();
        loop {
            if job.id() == critical_to_job.id() {
                inner.critical_to_job = Some((job, retcode_nonzero));
                return Ok(());
            }
            if let Some(p) = job.parent() {
                job = p;
            } else {
                break;
            }
        }
        Err(ZxError::INVALID_ARGS)
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
        self.inner.lock().remove_handle(handle_value)
    }

    /// Remove all handles from the process.
    ///
    /// If one or more error happens, return one of them.
    /// All handles are discarded on success or failure.
    pub fn remove_handles(&self, handle_values: &[HandleValue]) -> ZxResult<Vec<Handle>> {
        let mut inner = self.inner.lock();
        handle_values
            .iter()
            .map(|h| inner.remove_handle(*h))
            .collect()
    }

    pub fn remove_object<T: KernelObject>(&self, handle_value: HandleValue) -> ZxResult<Arc<T>> {
        let handle = self.remove_handle(handle_value)?;
        let object = handle
            .object
            .downcast_arc::<T>()
            .map_err(|_| ZxError::WRONG_TYPE)?;
        Ok(object)
    }

    /// Get a handle from the process
    fn get_handle(&self, handle_value: HandleValue) -> ZxResult<Handle> {
        self.inner.lock().get_handle(handle_value)
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
            Some((h, _)) => h.clone(),
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

    pub fn get_dyn_object_and_rights(
        &self,
        handle_value: HandleValue,
    ) -> ZxResult<(Arc<dyn KernelObject>, Rights)> {
        let handle = self.get_handle(handle_value)?;
        Ok((handle.object, handle.rights))
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

    pub fn get_cancel_token(&self, handle_value: HandleValue) -> ZxResult<Receiver<()>> {
        self.inner.lock().get_cancel_token(handle_value)
    }

    pub fn get_exceptionate(&self) -> Arc<Exceptionate> {
        self.exceptionate.clone()
    }

    /// Get KoIDs of Threads.
    pub fn thread_ids(&self) -> Vec<KoID> {
        self.inner.lock().threads.iter().map(|t| t.id()).collect()
    }
}

impl Task for Process {
    fn kill(&self) {
        let retcode = TASK_RETCODE_SYSCALL_KILL;
        self.exit(retcode);
    }

    fn suspend(&self) {
        let inner = self.inner.lock();
        for thread in inner.threads.iter() {
            thread.suspend();
        }
    }

    fn resume(&self) {
        let inner = self.inner.lock();
        for thread in inner.threads.iter() {
            thread.resume();
        }
    }

    fn create_exception_channel(&mut self, _options: u32) -> ZxResult<Channel> {
        unimplemented!();
    }

    fn resume_from_exception(&mut self, _port: &Port, _options: u32) -> ZxResult {
        unimplemented!();
    }
}

impl ProcessInner {
    /// Add a handle to the process
    fn add_handle(&mut self, handle: Handle) -> HandleValue {
        // FIXME: handle value from ptr
        let key = (self.max_handle_id << 2) | 0x3u32;
        info!("add handle: {:#x}, {:?}", key, handle.object);
        self.max_handle_id += 1;
        self.handles.insert(key, (handle, Vec::new()));
        key
    }

    /// Whether `thread` is in this process.
    fn contains_thread(&self, thread: &Arc<Thread>) -> bool {
        self.threads.iter().any(|t| Arc::ptr_eq(t, thread))
    }

    fn remove_handle(&mut self, handle_value: HandleValue) -> ZxResult<Handle> {
        let (handle, queue) = self
            .handles
            .remove(&handle_value)
            .ok_or(ZxError::BAD_HANDLE)?;
        for sender in queue {
            let _ = sender.send(());
        }
        Ok(handle)
    }

    fn get_cancel_token(&mut self, handle_value: HandleValue) -> ZxResult<Receiver<()>> {
        let (_, queue) = self
            .handles
            .get_mut(&handle_value)
            .ok_or(ZxError::BAD_HANDLE)?;
        let (sender, receiver) = oneshot::channel();
        queue.push(sender);
        Ok(receiver)
    }

    fn get_handle(&mut self, handle_value: HandleValue) -> ZxResult<Handle> {
        let (handle, _) = self.handles.get(&handle_value).ok_or(ZxError::BAD_HANDLE)?;
        Ok(handle.clone())
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
