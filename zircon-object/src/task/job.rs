use {
    super::exception::*,
    super::job_policy::*,
    super::process::Process,
    crate::object::*,
    crate::task::Task,
    alloc::sync::{Arc, Weak},
    alloc::vec::Vec,
    lock::Mutex,
};

/// Control a group of processes
///
/// ## SYNOPSIS
///
/// A job is a group of processes and possibly other (child) jobs. Jobs are used to
/// track privileges to perform kernel operations (i.e., make various syscalls,
/// with various options), and track and limit basic resource (e.g., memory, CPU)
/// consumption. Every process belongs to a single job. Jobs can also be nested,
/// and every job except the root job also belongs to a single (parent) job.
///
/// ## DESCRIPTION
///
/// A job is an object consisting of the following:
/// - a reference to a parent job
/// - a set of child jobs (each of whom has this job as parent)
/// - a set of member [processes](crate::task::Process)
/// - a set of policies
///
/// Jobs control "applications" that are composed of more than one process to be
/// controlled as a single entity.
#[allow(dead_code)]
pub struct Job {
    base: KObjectBase,
    _counter: CountHelper,
    parent: Option<Arc<Job>>,
    parent_policy: JobPolicy,
    exceptionate: Arc<Exceptionate>,
    debug_exceptionate: Arc<Exceptionate>,
    inner: Mutex<JobInner>,
}

impl_kobject!(Job
    fn get_child(&self, id: KoID) -> ZxResult<Arc<dyn KernelObject>> {
        let inner = self.inner.lock();
        if let Some(job) = inner.children.iter().filter_map(|o|o.upgrade()).find(|o| o.id() == id) {
            return Ok(job);
        }
        if let Some(proc) = inner.processes.iter().find(|o| o.id() == id) {
            return Ok(proc.clone());
        }
        Err(ZxError::NOT_FOUND)
    }
    fn related_koid(&self) -> KoID {
        self.parent.as_ref().map(|p| p.id()).unwrap_or(0)
    }
);
define_count_helper!(Job);

#[derive(Default)]
struct JobInner {
    policy: JobPolicy,
    children: Vec<Weak<Job>>,
    processes: Vec<Arc<Process>>,
    // if the job is killed, no more child creation should works
    killed: bool,
    timer_policy: TimerSlack,
    self_ref: Weak<Job>,
}

impl Job {
    /// Create the root job.
    pub fn root() -> Arc<Self> {
        let job = Arc::new(Job {
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            parent: None,
            parent_policy: JobPolicy::default(),
            exceptionate: Exceptionate::new(ExceptionChannelType::Job),
            debug_exceptionate: Exceptionate::new(ExceptionChannelType::JobDebugger),
            inner: Mutex::new(JobInner::default()),
        });
        job.inner.lock().self_ref = Arc::downgrade(&job);
        job
    }

    /// Create a new child job object.
    pub fn create_child(self: &Arc<Self>) -> ZxResult<Arc<Self>> {
        let mut inner = self.inner.lock();
        if inner.killed {
            return Err(ZxError::BAD_STATE);
        }
        let child = Arc::new(Job {
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            parent: Some(self.clone()),
            parent_policy: inner.policy.merge(&self.parent_policy),
            exceptionate: Exceptionate::new(ExceptionChannelType::Job),
            debug_exceptionate: Exceptionate::new(ExceptionChannelType::JobDebugger),
            inner: Mutex::new(JobInner::default()),
        });
        let child_weak = Arc::downgrade(&child);
        child.inner.lock().self_ref = child_weak.clone();
        inner.children.push(child_weak);
        Ok(child)
    }

    fn remove_child(&self, to_remove: &Weak<Job>) {
        let mut inner = self.inner.lock();
        inner.children.retain(|child| !to_remove.ptr_eq(child));
        if inner.killed && inner.processes.is_empty() && inner.children.is_empty() {
            drop(inner);
            self.terminate()
        }
    }

    /// Get the policy of the job.
    pub fn policy(&self) -> JobPolicy {
        self.inner.lock().policy.merge(&self.parent_policy)
    }

    /// Get the parent job.
    pub fn parent(&self) -> Option<Arc<Self>> {
        self.parent.clone()
    }

    /// Sets one or more security and/or resource policies to an empty job.
    ///
    /// The job's effective policies is the combination of the parent's
    /// effective policies and the policies specified in policy.
    ///
    /// After this call succeeds any new child process or child job will have
    /// the new effective policy applied to it.
    pub fn set_policy_basic(
        &self,
        options: SetPolicyOptions,
        policies: &[BasicPolicy],
    ) -> ZxResult {
        let mut inner = self.inner.lock();
        if !inner.is_empty() {
            return Err(ZxError::BAD_STATE);
        }
        for policy in policies {
            if self.parent_policy.get_action(policy.condition).is_some() {
                match options {
                    SetPolicyOptions::Absolute => return Err(ZxError::ALREADY_EXISTS),
                    SetPolicyOptions::Relative => {}
                }
            } else {
                inner.policy.apply(*policy);
            }
        }
        Ok(())
    }

    /// Sets timer slack policy to an empty job.
    pub fn set_policy_timer_slack(&self, policy: TimerSlackPolicy) -> ZxResult {
        let mut inner = self.inner.lock();
        if !inner.is_empty() {
            return Err(ZxError::BAD_STATE);
        }
        check_timer_policy(&policy)?;
        inner.timer_policy = inner.timer_policy.generate_new(policy);
        Ok(())
    }

    /// Add a process to the job.
    pub(super) fn add_process(&self, process: Arc<Process>) -> ZxResult {
        let mut inner = self.inner.lock();
        if inner.killed {
            return Err(ZxError::BAD_STATE);
        }
        inner.processes.push(process);
        Ok(())
    }

    /// Remove a process from the job.
    pub(super) fn remove_process(&self, id: KoID) {
        let mut inner = self.inner.lock();
        inner.processes.retain(|proc| proc.id() != id);
        if inner.killed && inner.processes.is_empty() && inner.children.is_empty() {
            drop(inner);
            self.terminate()
        }
    }

    /// Get information of this job.
    pub fn get_info(&self) -> JobInfo {
        JobInfo::default()
    }

    /// Check whether this job is root job.
    pub fn check_root_job(&self) -> ZxResult {
        if self.parent.is_some() {
            Err(ZxError::ACCESS_DENIED)
        } else {
            Ok(())
        }
    }

    /// Get KoIDs of Processes.
    pub fn process_ids(&self) -> Vec<KoID> {
        self.inner.lock().processes.iter().map(|p| p.id()).collect()
    }

    /// Get KoIDs of children Jobs.
    pub fn children_ids(&self) -> Vec<KoID> {
        self.inner
            .lock()
            .children
            .iter()
            .filter_map(|j| j.upgrade())
            .map(|j| j.id())
            .collect()
    }

    /// Return true if this job has no processes and no child jobs.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }

    /// The job finally terminates.
    fn terminate(&self) {
        self.exceptionate.shutdown();
        self.debug_exceptionate.shutdown();
        self.base.signal_set(Signal::JOB_TERMINATED);
        if let Some(parent) = self.parent.as_ref() {
            parent.remove_child(&self.inner.lock().self_ref)
        }
    }
}

impl Task for Job {
    /// Kill the job. The job do not terminate immediately when killed.
    /// It will terminate after all its children and processes are terminated.
    fn kill(&self) {
        let (children, processes) = {
            let mut inner = self.inner.lock();
            if inner.killed {
                return;
            }
            inner.killed = true;
            (inner.children.clone(), inner.processes.clone())
        };
        if children.is_empty() && processes.is_empty() {
            self.terminate();
            return;
        }
        for child in children {
            if let Some(child) = child.upgrade() {
                child.kill();
            }
        }
        for proc in processes {
            proc.kill();
        }
    }

    fn suspend(&self) {
        panic!("job do not support suspend");
    }

    fn resume(&self) {
        panic!("job do not support resume");
    }

    fn exceptionate(&self) -> Arc<Exceptionate> {
        self.exceptionate.clone()
    }

    fn debug_exceptionate(&self) -> Arc<Exceptionate> {
        self.debug_exceptionate.clone()
    }
}

impl JobInner {
    fn is_empty(&self) -> bool {
        self.processes.is_empty() && self.children.is_empty()
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// Information of a job.
#[repr(C)]
#[derive(Default)]
pub struct JobInfo {
    return_code: i64,
    exited: bool,
    kill_on_oom: bool,
    debugger_attached: bool,
    padding: [u8; 5],
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Status, Thread, ThreadState, TASK_RETCODE_SYSCALL_KILL};
    use core::time::Duration;

    #[test]
    fn create() {
        let root_job = Job::root();
        let job = Job::create_child(&root_job).expect("failed to create job");

        let child = root_job
            .get_child(job.id())
            .unwrap()
            .downcast_arc()
            .unwrap();
        assert!(Arc::ptr_eq(&child, &job));
        assert_eq!(job.related_koid(), root_job.id());
        assert_eq!(root_job.related_koid(), 0);

        root_job.kill();
        assert_eq!(root_job.create_child().err(), Some(ZxError::BAD_STATE));
    }

    #[test]
    fn set_policy() {
        let root_job = Job::root();

        // default policy
        assert_eq!(
            root_job.policy().get_action(PolicyCondition::BadHandle),
            None
        );

        // set policy for root job
        let policy = &[BasicPolicy {
            condition: PolicyCondition::BadHandle,
            action: PolicyAction::Deny,
        }];
        root_job
            .set_policy_basic(SetPolicyOptions::Relative, policy)
            .expect("failed to set policy");
        assert_eq!(
            root_job.policy().get_action(PolicyCondition::BadHandle),
            Some(PolicyAction::Deny)
        );

        // override policy should success
        let policy = &[BasicPolicy {
            condition: PolicyCondition::BadHandle,
            action: PolicyAction::Allow,
        }];
        root_job
            .set_policy_basic(SetPolicyOptions::Relative, policy)
            .expect("failed to set policy");
        assert_eq!(
            root_job.policy().get_action(PolicyCondition::BadHandle),
            Some(PolicyAction::Allow)
        );

        // create a child job
        let job = Job::create_child(&root_job).expect("failed to create job");

        // should inherit parent's policy.
        assert_eq!(
            job.policy().get_action(PolicyCondition::BadHandle),
            Some(PolicyAction::Allow)
        );

        // setting policy for a non-empty job should fail.
        assert_eq!(
            root_job.set_policy_basic(SetPolicyOptions::Relative, &[]),
            Err(ZxError::BAD_STATE)
        );

        // set new policy should success.
        let policy = &[BasicPolicy {
            condition: PolicyCondition::WrongObject,
            action: PolicyAction::Allow,
        }];
        job.set_policy_basic(SetPolicyOptions::Relative, policy)
            .expect("failed to set policy");
        assert_eq!(
            job.policy().get_action(PolicyCondition::WrongObject),
            Some(PolicyAction::Allow)
        );

        // relatively setting existing policy should be ignored.
        let policy = &[BasicPolicy {
            condition: PolicyCondition::BadHandle,
            action: PolicyAction::Deny,
        }];
        job.set_policy_basic(SetPolicyOptions::Relative, policy)
            .expect("failed to set policy");
        assert_eq!(
            job.policy().get_action(PolicyCondition::BadHandle),
            Some(PolicyAction::Allow)
        );

        // absolutely setting existing policy should fail.
        assert_eq!(
            job.set_policy_basic(SetPolicyOptions::Absolute, policy),
            Err(ZxError::ALREADY_EXISTS)
        );
    }

    #[test]
    fn parent_child() {
        let root_job = Job::root();
        let job = Job::create_child(&root_job).expect("failed to create job");
        let proc = Process::create(&root_job, "proc").expect("failed to create process");

        assert_eq!(root_job.get_child(job.id()).unwrap().id(), job.id());
        assert_eq!(root_job.get_child(proc.id()).unwrap().id(), proc.id());
        assert_eq!(
            root_job.get_child(root_job.id()).err(),
            Some(ZxError::NOT_FOUND)
        );
        assert!(Arc::ptr_eq(&job.parent().unwrap(), &root_job));

        let job1 = root_job.create_child().expect("failed to create job");
        let proc1 = Process::create(&root_job, "proc1").expect("failed to create process");
        assert_eq!(root_job.children_ids(), vec![job.id(), job1.id()]);
        assert_eq!(root_job.process_ids(), vec![proc.id(), proc1.id()]);

        root_job.kill();
        assert_eq!(root_job.create_child().err(), Some(ZxError::BAD_STATE));
    }

    #[test]
    fn check() {
        let root_job = Job::root();
        assert!(root_job.is_empty());
        let job = root_job.create_child().expect("failed to create job");
        assert_eq!(root_job.check_root_job(), Ok(()));
        assert_eq!(job.check_root_job(), Err(ZxError::ACCESS_DENIED));

        assert!(!root_job.is_empty());
        assert!(job.is_empty());

        let _proc = Process::create(&job, "proc").expect("failed to create process");
        assert!(!job.is_empty());
    }

    #[async_std::test]
    async fn kill() {
        let root_job = Job::root();
        let job = Job::create_child(&root_job).expect("failed to create job");
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        let thread = Thread::create(&proc, "thread").expect("failed to create thread");
        thread
            .start(|thread| {
                std::boxed::Box::pin(async {
                    println!("should not be killed");
                    async_std::task::sleep(Duration::from_millis(1000)).await;
                    {
                        // FIXME
                        drop(thread);
                        async_std::task::sleep(Duration::from_millis(1000)).await;
                    }
                    unreachable!("should be killed");
                })
            })
            .expect("failed to start thread");

        async_std::task::sleep(Duration::from_millis(500)).await;
        root_job.kill();
        assert!(root_job.inner.lock().killed);
        assert!(job.inner.lock().killed);
        assert_eq!(proc.status(), Status::Exited(TASK_RETCODE_SYSCALL_KILL));
        assert_eq!(thread.state(), ThreadState::Dying);
        // killed but not terminated, since `CurrentThread` not dropped.
        assert!(!root_job.signal().contains(Signal::JOB_TERMINATED));
        assert!(job.signal().contains(Signal::JOB_TERMINATED)); // but the lonely job is terminated
        assert!(!proc.signal().contains(Signal::PROCESS_TERMINATED));
        assert!(!thread.signal().contains(Signal::THREAD_TERMINATED));

        // wait for killing...
        async_std::task::sleep(Duration::from_millis(1000)).await;
        assert!(root_job.inner.lock().killed);
        assert!(job.inner.lock().killed);
        assert_eq!(proc.status(), Status::Exited(TASK_RETCODE_SYSCALL_KILL));
        assert_eq!(thread.state(), ThreadState::Dead);
        // all terminated now
        assert!(root_job.signal().contains(Signal::JOB_TERMINATED));
        assert!(job.signal().contains(Signal::JOB_TERMINATED));
        assert!(proc.signal().contains(Signal::PROCESS_TERMINATED));
        assert!(thread.signal().contains(Signal::THREAD_TERMINATED));

        // The job has no children.
        let root_job = Job::root();
        root_job.kill();
        assert!(root_job.inner.lock().killed);
        assert!(root_job.signal().contains(Signal::JOB_TERMINATED));

        // The job's process have no threads.
        let root_job = Job::root();
        let job = Job::create_child(&root_job).expect("failed to create job");
        let proc = Process::create(&root_job, "proc").expect("failed to create process");
        root_job.kill();
        assert!(root_job.inner.lock().killed);
        assert!(job.inner.lock().killed);
        assert_eq!(proc.status(), Status::Exited(TASK_RETCODE_SYSCALL_KILL));
        assert!(root_job.signal().contains(Signal::JOB_TERMINATED));
        assert!(job.signal().contains(Signal::JOB_TERMINATED));
        assert!(proc.signal().contains(Signal::PROCESS_TERMINATED));
    }

    #[test]
    fn critical_process() {
        let root_job = Job::root();
        let job = root_job.create_child().unwrap();
        let job1 = root_job.create_child().unwrap();

        let proc = Process::create(&job, "proc").expect("failed to create process");

        assert_eq!(
            proc.set_critical_at_job(&job1, true).err(),
            Some(ZxError::INVALID_ARGS)
        );
        proc.set_critical_at_job(&root_job, true).unwrap();
        assert_eq!(
            proc.set_critical_at_job(&job, true).err(),
            Some(ZxError::ALREADY_BOUND)
        );

        proc.exit(666);
        assert!(root_job.inner.lock().killed);
        assert!(root_job.signal().contains(Signal::JOB_TERMINATED));
    }
}
