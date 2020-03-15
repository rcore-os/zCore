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

        const USER_ALL                      = 0xff << 24;

        const CLOCK_STARTED                 = 1 << 4;

        const SOCKET_PEER_WRITE_DISABLED    = 1 << 4;
        const SOCKET_WRITE_DISABLED         = 1 << 5;
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

        // for Linux
        const SIGCHLD                       = 1 << 6;
    }
}

impl Signal {
    pub fn verify_user_signal(number: u32) -> ZxResult<Signal> {
        if (number & !Signal::USER_ALL.bits()) != 0 {
            Err(ZxError::INVALID_ARGS)
        } else {
            Ok(Signal::from_bits(number).ok_or(ZxError::INVALID_ARGS)?)
        }
    }
}
