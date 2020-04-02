use {
    super::job_policy::*, super::process::Process, crate::object::*, alloc::sync::Arc,
    alloc::vec::Vec, spin::Mutex,
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
pub struct Job {
    base: KObjectBase,
    parent: Option<Arc<Job>>,
    parent_policy: JobPolicy,
    inner: Mutex<JobInner>,
}

impl_kobject!(Job
    fn get_child(&self, id: KoID) -> ZxResult<Arc<dyn KernelObject>> {
        let inner = self.inner.lock();
        if let Some(job) = inner.children.iter().find(|o| o.id() == id) {
            return Ok(job.clone());
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

#[derive(Default)]
struct JobInner {
    policy: JobPolicy,
    children: Vec<Arc<Job>>,
    processes: Vec<Arc<Process>>,
    critical_proc: Option<(KoID, bool)>,
    timer_policy: TimerSlack,
}

impl Job {
    /// Create the root job.
    pub fn root() -> Arc<Self> {
        Arc::new(Job {
            base: KObjectBase::new(),
            parent: None,
            parent_policy: JobPolicy::default(),
            inner: Mutex::new(JobInner::default()),
        })
    }

    /// Create a new child job object.
    pub fn create_child(self: &Arc<Self>, _options: u32) -> ZxResult<Arc<Self>> {
        // TODO: options
        let mut inner = self.inner.lock();
        let child = Arc::new(Job {
            base: KObjectBase::new(),
            parent: Some(self.clone()),
            parent_policy: inner.policy.merge(&self.parent_policy),
            inner: Mutex::new(JobInner::default()),
        });
        inner.children.push(child.clone());
        Ok(child)
    }

    /// Get the policy of the job.
    pub fn policy(&self) -> JobPolicy {
        self.inner.lock().policy.merge(&self.parent_policy)
    }

    /// Sets one or more security and/or resource policies to an empty job.
    ///
    /// The job's effective policies is the combination of the parent's
    /// effective policies and the policies specified in policy.
    ///
    /// After this call succeeds any new child process or child job will have
    /// the new effective policy applied to it.
    pub fn set_policy_basic(&self, options: SetPolicyOptions, policys: &[BasicPolicy]) -> ZxResult {
        let mut inner = self.inner.lock();
        if !inner.is_empty() {
            return Err(ZxError::BAD_STATE);
        }
        for policy in policys {
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

    pub fn set_policy_timer_slack(&self, policy: TimerSlackPolicy) -> ZxResult{
        let mut inner = self.inner.lock();
        if !inner.is_empty() {
            return Err(ZxError::BAD_STATE);
        }
        check_timer_policy(&policy)?;
        inner.timer_policy = inner.timer_policy.generate_new(policy);
        Ok(())
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
    pub fn set_critical(&self, proc: &Arc<Process>, retcode_nonzero: bool) -> ZxResult {
        let mut inner = self.inner.lock();
        if let &Some((pid, _)) = &inner.critical_proc {
            if proc.id() == pid {
                return Err(ZxError::ALREADY_BOUND);
            }
        }
        if !inner.processes.iter().any(|p| proc.id() == p.id()) {
            return Err(ZxError::INVALID_ARGS);
        }
        inner.critical_proc = Some((proc.id(), retcode_nonzero));
        Ok(())
    }

    /// Add a process to the job.
    pub(super) fn add_process(&self, process: Arc<Process>) {
        self.inner.lock().processes.push(process);
    }

    pub(super) fn process_exit(&self, id: KoID, retcode: i64) {
        let mut inner = self.inner.lock();
        inner.processes.retain(|proc| proc.id() != id);
        if let &Some((pid, retcode_nonzero)) = &inner.critical_proc {
            if pid == id && !(retcode_nonzero && retcode == 0) {
                unimplemented!("kill the job")
            }
        }
    }

    pub fn get_info(&self) -> JobInfo {
        JobInfo::default()
    }
}

impl JobInner {
    fn is_empty(&self) -> bool {
        self.processes.is_empty() && self.children.is_empty()
    }
}

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

    #[test]
    fn create() {
        let root_job = Job::root();
        let _job = Job::create_child(&root_job, 0).expect("failed to create job");
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
        let job = Job::create_child(&root_job, 0).expect("failed to create job");

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
    fn get_child() {
        let root_job = Job::root();
        let job = Job::create_child(&root_job, 0).expect("failed to create job");
        let proc = Process::create(&root_job, "proc", 0).expect("failed to create process");

        let root_job: Arc<dyn KernelObject> = root_job;
        assert_eq!(root_job.get_child(job.id()).unwrap().id(), job.id());
        assert_eq!(root_job.get_child(proc.id()).unwrap().id(), proc.id());
        assert_eq!(
            root_job.get_child(root_job.id()).err(),
            Some(ZxError::NOT_FOUND)
        );
    }
}
