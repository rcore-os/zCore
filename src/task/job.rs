use super::job_policy::*;
use crate::object::KObjectBase;
use alloc::collections::BTreeSet;
use alloc::sync::{Arc, Weak};

pub struct Job {
    base: KObjectBase,
    pub(super) policy: JobPolicy,
    parent: Option<Arc<Job>>,
    children: BTreeSet<Weak<Job>>,
}

impl Job {
    /// Create a new child job object.
    pub fn create_child(&mut self, _options: u32) -> Self {
        unimplemented!()
    }

    /// Sets one or more security and/or resource policies to an empty job.
    ///
    /// The job's effective policies is the combination of the parent's
    /// effective policies and the policies specified in policy.
    ///
    /// After this call succeeds any new child process or child job will have
    /// the new effective policy applied to it.
    pub fn set_policy_basic(&mut self, _options: SetPolicyOptions, _policys: &[BasicPolicy]) {
        unimplemented!()
    }

    pub fn set_policy_timer_slack(
        &mut self,
        _options: SetPolicyOptions,
        _policys: &[TimerSlackPolicy],
    ) {
        unimplemented!()
    }
}
