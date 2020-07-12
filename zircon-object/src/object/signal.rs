#![allow(missing_docs)]
use {super::*, bitflags::bitflags};

bitflags! {
    /// Signals that waitable kernel objects expose to applications.
    #[derive(Default)]
    pub struct Signal: u32 {
        #[allow(clippy::identity_op)]
        const READABLE                      = 1 << 0;
        const WRITABLE                      = 1 << 1;
        const PEER_CLOSED                   = 1 << 2;
        const SIGNALED                      = 1 << 3;
        const HANDLE_CLOSED                 = 1 << 23;

        const KERNEL_ALL                    = 0xff_ffff;
        const USER_ALL                      = 0xff << 24;

        const CLOCK_STARTED                 = 1 << 4;

        const SOCKET_PEER_WRITE_DISABLED    = 1 << 4;
        const SOCKET_WRITE_DISABLED         = 1 << 5;
        const SOCKET_CONTROL_READABLE       = 1 << 6;
        const SOCKET_CONTROL_WRITABLE       = 1 << 7;
        const SCOEKT_ACCEPT                 = 1 << 8;
        const SOCKET_SHARE                  = 1 << 9;
        const SOCKET_READ_THRESHOLD         = 1 << 10;
        const SOCKET_WRITE_THRESHOLD        = 1 << 11;


        const TASK_TERMINATED               = Self::SIGNALED.bits;

        const JOB_TERMINATED                = Self::SIGNALED.bits;
        const JOB_NO_JOBS                   = 1 << 4;
        const JOB_NO_PROCESSES              = 1 << 5;

        const PROCESS_TERMINATED            = Self::SIGNALED.bits;

        const THREAD_TERMINATED             = Self::SIGNALED.bits;
        const THREAD_RUNNING                = 1 << 4;
        const THREAD_SUSPENDED              = 1 << 5;

        const VMO_ZERO_CHILDREN             = Self::SIGNALED.bits;

        const INTERRUPT_SIGNAL              = 1 << 4;

        // for Linux
        const SIGCHLD                       = 1 << 6;

        // for user
        const USER_SIGNAL_0                 = 1 << 24;
        const USER_SIGNAL_1                 = 1 << 25;
        const USER_SIGNAL_2                 = 1 << 26;
        const USER_SIGNAL_3                 = 1 << 27;
        const USER_SIGNAL_4                 = 1 << 28;
        const USER_SIGNAL_5                 = 1 << 29;
        const USER_SIGNAL_6                 = 1 << 30;
        const USER_SIGNAL_7                 = 1 << 31;
    }
}

impl Signal {
    /// Verify whether `number` only sets the bits specified in `allowed_signals`.
    pub fn verify_user_signal(allowed_signals: Signal, number: u32) -> ZxResult<Signal> {
        if (number & !allowed_signals.bits()) != 0 {
            Err(ZxError::INVALID_ARGS)
        } else {
            Ok(Signal::from_bits(number).ok_or(ZxError::INVALID_ARGS)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_user_signal() {
        assert_eq!(
            Err(ZxError::INVALID_ARGS),
            Signal::verify_user_signal(Signal::USER_ALL, 1 << 0)
        );

        assert_eq!(
            Ok(Signal::USER_SIGNAL_0),
            Signal::verify_user_signal(Signal::USER_ALL, 1 << 24)
        );
    }
}
