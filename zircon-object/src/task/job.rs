use super::job_policy::*;
use super::process::Process;
use crate::object::*;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

pub struct Job {
    base: KObjectBase,
    _parent: Option<Arc<Job>>,
    parent_policy: JobPolicy,
    inner: Mutex<JobInner>,
}

impl_kobject!(Job);

#[derive(Default)]
struct JobInner {
    policy: JobPolicy,
    children: Vec<Arc<Job>>,
    processes: Vec<Arc<Process>>,
}

impl Job {
    /// Create the root job.
    pub fn root() -> Arc<Self> {
        Arc::new(Job {
            base: KObjectBase::new(),
            _parent: None,
            parent_policy: JobPolicy::default(),
            inner: Mutex::new(JobInner::default()),
        })
    }

    /// Create a new child job object.
    pub fn create_child(parent: &Arc<Self>, _options: u32) -> ZxResult<Arc<Self>> {
        // TODO: options
        let mut inner = parent.inner.lock();
        let child = Arc::new(Job {
            base: KObjectBase::new(),
            _parent: Some(parent.clone()),
            parent_policy: inner.policy.merge(&parent.parent_policy),
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
    pub fn set_policy_basic(
        &self,
        options: SetPolicyOptions,
        policys: &[BasicPolicy],
    ) -> ZxResult<()> {
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

    pub fn set_policy_timer_slack(
        &self,
        _options: SetPolicyOptions,
        _policys: &[TimerSlackPolicy],
    ) {
        unimplemented!()
    }

    /// Add a process to the job.
    pub(super) fn add_process(&self, process: Arc<Process>) {
        self.inner.lock().processes.push(process);
    }
}

impl JobInner {
    fn is_empty(&self) -> bool {
        self.processes.is_empty() && self.children.is_empty()
    }
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
}
