use crate::error::*;
use crate::signal::Slack;

/// Security and resource policies of a job.
#[derive(Default, Copy, Clone)]
pub struct JobPolicy {
    // TODO: use bitset
    action: [Option<PolicyAction>; 15],
}

impl JobPolicy {
    /// Get the action of a policy `condition`.
    pub fn get_action(&self, condition: PolicyCondition) -> Option<PolicyAction> {
        self.action[condition as usize]
    }

    /// Apply a basic policy.
    pub fn apply(&mut self, policy: BasicPolicy) {
        self.action[policy.condition as usize] = Some(policy.action);
    }

    /// Merge the policy with `parent`'s.
    pub fn merge(&self, parent: &Self) -> Self {
        let mut new = *self;
        for i in 0..15 {
            if parent.action[i].is_some() {
                new.action[i] = parent.action[i];
            }
        }
        new
    }
}

/// Control the effect in the case of conflict between
/// the existing policies and the new policies when setting new policies.
#[derive(Debug, Copy, Clone)]
pub enum SetPolicyOptions {
    /// Policy is applied for all conditions in policy or the call fails.
    Absolute,
    /// Policy is applied for the conditions not specifically overridden by the parent policy.
    Relative,
}

/// The policy type.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct BasicPolicy {
    /// Condition when the policy is applied.
    pub condition: PolicyCondition,
    ///
    pub action: PolicyAction,
}

/// The condition when a policy is applied.
#[repr(u32)]
#[derive(Debug, Copy, Clone)]
pub enum PolicyCondition {
    /// A process under this job is attempting to issue a syscall with an invalid handle.
    /// In this case, `PolicyAction::Allow` and `PolicyAction::Deny` are equivalent:
    /// if the syscall returns, it will always return the error ZX_ERR_BAD_HANDLE.
    BadHandle = 0,
    /// A process under this job is attempting to issue a syscall with a handle that does not support such operation.
    WrongObject = 1,
    /// A process under this job is attempting to map an address region with write-execute access.
    VmarWx = 2,
    /// A special condition that stands for all of the above ZX_NEW conditions
    /// such as NEW_VMO, NEW_CHANNEL, NEW_EVENT, NEW_EVENTPAIR, NEW_PORT, NEW_SOCKET, NEW_FIFO,
    /// And any future ZX_NEW policy.
    /// This will include any new kernel objects which do not require a parent object for creation.
    NewAny = 3,
    /// A process under this job is attempting to create a new vm object.
    NewVMO = 4,
    /// A process under this job is attempting to create a new channel.
    NewChannel = 5,
    /// A process under this job is attempting to create a new event.
    NewEvent = 6,
    /// A process under this job is attempting to create a new event pair.
    NewEventPair = 7,
    /// A process under this job is attempting to create a new port.
    NewPort = 8,
    /// A process under this job is attempting to create a new socket.
    NewSocket = 9,
    /// A process under this job is attempting to create a new fifo.
    NewFIFO = 10,
    /// A process under this job is attempting to create a new timer.
    NewTimer = 11,
    /// A process under this job is attempting to create a new process.
    NewProcess = 12,
    /// A process under this job is attempting to create a new profile.
    NewProfile = 13,
    /// A process under this job is attempting to use zx_vmo_replace_as_executable()
    /// with a ZX_HANDLE_INVALID as the second argument rather than a valid ZX_RSRC_KIND_VMEX.
    AmbientMarkVMOExec = 14,
}

/// The action taken when the condition happens specified by a policy.
#[repr(u32)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum PolicyAction {
    /// Allow condition.
    Allow = 0,
    /// Prevent condition.
    Deny = 1,
    /// Generate an exception via the debug port. An exception generated this
    /// way acts as a breakpoint. The thread may be resumed after the exception.
    AllowException = 2,
    /// Just like `AllowException`, but after resuming condition is denied.
    DenyException = 3,
    /// Terminate the process.
    Kill = 4,
}

/// Timer slack policy.
///
/// See [timer slack](../../signal/timer/enum.Slack.html) for more information.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TimerSlackPolicy {
    min_slack: i64,
    default_mode: Slack,
}

/// Check whether the policy is valid.
pub fn check_timer_policy(policy: &TimerSlackPolicy) -> ZxResult {
    if policy.min_slack.is_negative() {
        return Err(ZxError::INVALID_ARGS);
    }
    Ok(())
}

#[repr(C)]
pub(super) struct TimerSlack {
    amount: i64,
    mode: Slack,
}

impl TimerSlack {
    pub(super) fn generate_new(&self, policy: TimerSlackPolicy) -> TimerSlack {
        TimerSlack {
            amount: self.amount.max(policy.min_slack),
            mode: policy.default_mode,
        }
    }
}

impl Default for TimerSlack {
    fn default() -> Self {
        TimerSlack {
            amount: 0,
            mode: Slack::Center,
        }
    }
}
