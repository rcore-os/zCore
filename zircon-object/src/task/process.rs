use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{any::Any, sync::atomic::AtomicI32};

use futures::channel::oneshot::{self, Receiver, Sender};
use hashbrown::HashMap;
use lock::Mutex;

use super::exception::{ExceptionChannelType, Exceptionate};
use super::job_policy::{JobPolicy, PolicyAction, PolicyCondition};
use super::{Job, Task, Thread, ThreadFn};
use crate::object::{Handle, HandleBasicInfo, HandleValue, INVALID_HANDLE};
use crate::object::{KObjectBase, KernelObject, KoID, Rights, Signal};
use crate::{define_count_helper, impl_kobject};
use crate::{signal::Futex, vm::VmAddressRegion, ZxError, ZxResult};

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
    debug_exceptionate: Arc<Exceptionate>,
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

/// Status of a process.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Status {
    /// Initial state, no thread present in process.
    Init,
    /// First thread has started and is running.
    Running,
    /// Process has exited with the code.
    Exited(i64),
}

impl Default for Status {
    fn default() -> Self {
        Status::Init
    }
}

impl Process {
    /// Create a new process in the `job`.
    pub fn create(job: &Arc<Job>, name: &str) -> ZxResult<Arc<Self>> {
        Self::create_with_ext(job, name, ())
    }

    /// Create a new process with extension info.
    pub fn create_with_ext(
        job: &Arc<Job>,
        name: &str,
        ext: impl Any + Send + Sync,
    ) -> ZxResult<Arc<Self>> {
        let proc = Arc::new(Process {
            base: KObjectBase::with_name(name),
            _counter: CountHelper::new(),
            job: job.clone(),
            policy: job.policy(),
            vmar: VmAddressRegion::new_root(),
            ext: Box::new(ext),
            exceptionate: Exceptionate::new(ExceptionChannelType::Process),
            debug_exceptionate: Exceptionate::new(ExceptionChannelType::Debugger),
            inner: Mutex::new(ProcessInner::default()),
        });
        job.add_process(proc.clone())?;
        Ok(proc)
    }

    /// Start the first thread in the process.
    ///
    /// This causes a thread to begin execution at the program
    /// counter specified by `entry` and with the stack pointer set to `stack`.
    /// The arguments `arg1` and `arg2` are arranged to be in the architecture
    /// specific registers used for the first two arguments of a function call
    /// before the thread is started. All other registers are zero upon start.
    ///
    /// # Example
    /// ```
    /// # use std::sync::Arc;
    /// # use zircon_object::task::*;
    /// # use zircon_object::object::*;
    /// # kernel_hal::init();
    /// # async_std::task::block_on(async {
    /// let job = Job::root();
    /// let proc = Process::create(&job, "proc").unwrap();
    /// let thread = Thread::create(&proc, "thread").unwrap();
    /// let handle = Handle::new(proc.clone(), Rights::DEFAULT_PROCESS);
    ///
    /// // start the new thread
    /// proc.start(&thread, 1, 4, Some(handle), 2, |thread| Box::pin(async move {
    ///     let cx = thread.wait_for_run().await;
    ///     assert_eq!(cx.general().rip, 1);  // entry
    ///     assert_eq!(cx.general().rsp, 4);  // stack_top
    ///     assert_eq!(cx.general().rdi, 3);  // arg0 (handle)
    ///     assert_eq!(cx.general().rsi, 2);  // arg1
    ///     thread.put_context(cx);
    /// })).unwrap();
    ///
    /// # let object: Arc<dyn KernelObject> = thread.clone();
    /// # object.wait_signal(Signal::THREAD_TERMINATED).await;
    /// # });
    /// ```
    pub fn start(
        &self,
        thread: &Arc<Thread>,
        entry: usize,
        stack: usize,
        arg1: Option<Handle>,
        arg2: usize,
        thread_fn: ThreadFn,
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
        thread.set_first_thread();
        let res = thread.start_with_entry(entry, stack, handle_value as usize, arg2, thread_fn);
        if res.is_err() && handle_value != INVALID_HANDLE {
            self.inner.lock().remove_handle(handle_value).ok();
        }
        res
    }

    /// Exit current process with `retcode`.
    /// The process do not terminate immediately when exited.
    /// It will terminate after all its child threads are terminated.
    pub fn exit(&self, retcode: i64) {
        let mut inner = self.inner.lock();
        if let Status::Exited(_) = inner.status {
            return;
        }
        inner.status = Status::Exited(retcode);
        if inner.threads.is_empty() {
            inner.handles.clear();
            drop(inner);
            self.terminate();
            return;
        }
        for thread in inner.threads.iter() {
            thread.kill();
        }
        inner.handles.clear();
    }

    /// The process finally terminates.
    fn terminate(&self) {
        let mut inner = self.inner.lock();
        let retcode = match inner.status {
            Status::Exited(retcode) => retcode,
            _ => {
                inner.status = Status::Exited(0);
                0
            }
        };
        self.base.signal_set(Signal::PROCESS_TERMINATED);
        self.exceptionate.shutdown();
        self.debug_exceptionate.shutdown();

        self.job.remove_process(self.base.id);
        // If we are critical to a job, we need to take action.
        if let Some((job, retcode_nonzero)) = &inner.critical_to_job {
            if !retcode_nonzero || retcode != 0 {
                job.kill();
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

    /// Get process exit code if it exited, else returns `None`.
    pub fn exit_code(&self) -> Option<i64> {
        if let Status::Exited(code) = self.status() {
            Some(code)
        } else {
            None
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

    /// Remove a handle referring to a kernel object of the given type from the process.
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
        self.get_dyn_object_with_rights(handle_value, desired_rights)
            .and_then(|obj| obj.downcast_arc::<T>().map_err(|_| ZxError::WRONG_TYPE))
    }

    /// Get the kernel object corresponding to this `handle_value` and this handle's rights.
    pub fn get_object_and_rights<T: KernelObject>(
        &self,
        handle_value: HandleValue,
    ) -> ZxResult<(Arc<T>, Rights)> {
        let (object, rights) = self.get_dyn_object_and_rights(handle_value)?;
        let object = object
            .downcast_arc::<T>()
            .map_err(|_| ZxError::WRONG_TYPE)?;
        Ok((object, rights))
    }

    /// Get the kernel object corresponding to this `handle_value`,
    /// after checking that this handle has the `desired_rights`.
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

    /// Get the kernel object corresponding to this `handle_value` and this handle's rights.
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

    /// Get the handle's information corresponding to `handle_value`.
    pub fn get_handle_info(&self, handle_value: HandleValue) -> ZxResult<HandleBasicInfo> {
        let handle = self.get_handle(handle_value)?;
        Ok(handle.get_info())
    }

    /// Add a thread to the process.
    pub(super) fn add_thread(&self, thread: Arc<Thread>) -> ZxResult {
        let mut inner = self.inner.lock();
        if let Status::Exited(_) = inner.status {
            return Err(ZxError::BAD_STATE);
        }
        inner.threads.push(thread);
        Ok(())
    }

    /// Remove a thread from the process.
    ///
    /// If no more threads left, exit the process.
    pub(super) fn remove_thread(&self, tid: KoID) {
        let mut inner = self.inner.lock();
        inner.threads.retain(|t| t.id() != tid);
        if inner.threads.is_empty() {
            drop(inner);
            self.terminate();
        }
    }

    /// Get information of this process.
    pub fn get_info(&self) -> ProcessInfo {
        let mut info = ProcessInfo {
            debugger_attached: self.debug_exceptionate.has_channel(),
            ..Default::default()
        };
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
                info.started = true;
            }
        }
        info
    }

    /// Set the debug address.
    pub fn set_debug_addr(&self, addr: usize) {
        self.inner.lock().debug_addr = addr;
    }

    /// Get the debug address.
    pub fn get_debug_addr(&self) -> usize {
        self.inner.lock().debug_addr
    }

    /// Set the address where the dynamic loader will issue a debug trap on every load of a
    /// shared library to. Setting this property to
    /// zero will disable it.
    pub fn set_dyn_break_on_load(&self, addr: usize) {
        self.inner.lock().dyn_break_on_load = addr;
    }

    /// Get the address where the dynamic loader will issue a debug trap on every load of a
    /// shared library to.
    pub fn get_dyn_break_on_load(&self) -> usize {
        self.inner.lock().dyn_break_on_load
    }

    /// Get an one-shot `Receiver` for receiving cancel message of the given handle.
    pub fn get_cancel_token(&self, handle_value: HandleValue) -> ZxResult<Receiver<()>> {
        self.inner.lock().get_cancel_token(handle_value)
    }

    /// Get KoIDs of Threads.
    pub fn thread_ids(&self) -> Vec<KoID> {
        self.inner.lock().threads.iter().map(|t| t.id()).collect()
    }

    /// Wait for process exit and get return code.
    pub async fn wait_for_exit(self: &Arc<Self>) -> i64 {
        let object: Arc<dyn KernelObject> = self.clone();
        object.wait_signal(Signal::PROCESS_TERMINATED).await;
        let code = self.exit_code().expect("process not exited!");
        info!(
            "process {:?}({}) exited with code {:?}",
            self.name(),
            self.id(),
            code
        );
        code
    }
}

impl Task for Process {
    fn kill(&self) {
        self.exit(super::TASK_RETCODE_SYSCALL_KILL);
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

    fn exceptionate(&self) -> Arc<Exceptionate> {
        self.exceptionate.clone()
    }

    fn debug_exceptionate(&self) -> Arc<Exceptionate> {
        self.debug_exceptionate.clone()
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

/// Information of a process.
#[allow(missing_docs)]
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
    use crate::object::KernelObject;
    use crate::task::*;

    #[test]
    fn create() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");

        assert_eq!(proc.related_koid(), root_job.id());
        assert!(Arc::ptr_eq(&root_job, &proc.job()));
    }

    #[test]
    fn handle() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let handle = Handle::new(proc.clone(), Rights::DEFAULT_PROCESS);

        let handle_value = proc.add_handle(handle);
        let _info = proc.get_handle_info(handle_value).unwrap();

        // getting object should success
        let object: Arc<Process> = proc
            .get_object_with_rights(handle_value, Rights::DEFAULT_PROCESS)
            .expect("failed to get object");
        assert!(Arc::ptr_eq(&object, &proc));

        let (object, rights) = proc
            .get_object_and_rights::<Process>(handle_value)
            .expect("failed to get object");
        assert!(Arc::ptr_eq(&object, &proc));
        assert_eq!(rights, Rights::DEFAULT_PROCESS);

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

        let handle1 = Handle::new(proc.clone(), Rights::DEFAULT_PROCESS);
        let handle2 = Handle::new(proc.clone(), Rights::DEFAULT_PROCESS);

        let handle_values = proc.add_handles(vec![handle1, handle2]);
        let object1: Arc<Process> = proc
            .get_object_with_rights(handle_values[0], Rights::DEFAULT_PROCESS)
            .expect("failed to get object");
        assert!(Arc::ptr_eq(&object1, &proc));

        proc.remove_handles(&handle_values).unwrap();
        assert_eq!(
            proc.get_object_with_rights::<Process>(handle_values[0], Rights::DEFAULT_PROCESS)
                .err(),
            Some(ZxError::BAD_HANDLE)
        );
    }

    #[test]
    fn handle_duplicate() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");

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
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");

        assert_eq!(proc.get_child(thread.id()).unwrap().id(), thread.id());
        assert_eq!(proc.get_child(proc.id()).err(), Some(ZxError::NOT_FOUND));

        let thread1 = Thread::create(&proc, "thread1").expect("failed to create thread");
        assert_eq!(proc.thread_ids(), vec![thread.id(), thread1.id()]);
    }

    #[test]
    fn properties() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");

        proc.set_debug_addr(123);
        assert_eq!(proc.get_debug_addr(), 123);

        proc.set_dyn_break_on_load(2);
        assert_eq!(proc.get_dyn_break_on_load(), 2);
    }

    #[test]
    fn exit() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");

        let info = proc.get_info();
        assert!(!info.has_exited && !info.started && info.return_code == 0);

        proc.exit(666);
        let info = proc.get_info();
        assert!(info.has_exited && info.started && info.return_code == 666);
        assert_eq!(thread.state(), ThreadState::Dying);
        // TODO: when is the thread dead?

        assert_eq!(
            Thread::create(&proc, "thread1").err(),
            Some(ZxError::BAD_STATE)
        );
    }

    #[test]
    fn contains_thread() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");

        let proc1 = Process::create(&root_job, "proc1").expect("failed to create process");
        let thread1 = Thread::create(&proc1, "thread1").expect("failed to create thread");

        let inner = proc.inner.lock();
        assert!(inner.contains_thread(&thread) && !inner.contains_thread(&thread1));
    }

    #[test]
    fn check_policy() {
        let root_job = Job::root();
        let policy1 = BasicPolicy {
            condition: PolicyCondition::BadHandle,
            action: PolicyAction::Allow,
        };
        let policy2 = BasicPolicy {
            condition: PolicyCondition::NewChannel,
            action: PolicyAction::Deny,
        };

        assert!(root_job
            .set_policy_basic(SetPolicyOptions::Absolute, &[policy1, policy2])
            .is_ok());
        let proc = Process::create(&root_job, "proc").expect("failed to create process");

        assert!(proc.check_policy(PolicyCondition::BadHandle).is_ok());
        assert!(proc.check_policy(PolicyCondition::NewProcess).is_ok());
        assert_eq!(
            proc.check_policy(PolicyCondition::NewChannel).err(),
            Some(ZxError::ACCESS_DENIED)
        );

        let _job = root_job.create_child().unwrap();
        assert_eq!(
            root_job
                .set_policy_basic(SetPolicyOptions::Absolute, &[policy1, policy2])
                .err(),
            Some(ZxError::BAD_STATE)
        );
    }
}
