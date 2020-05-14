use {
    crate::{ZxError, ZxResult},
    bitflags::bitflags,
    core::convert::TryFrom,
};

bitflags! {
    /// Rights are associated with handles and convey privileges to perform actions on
    /// either the associated handle or the object associated with the handle.
    #[derive(Default)]
    pub struct Rights: u32 {
        /// Allows handle duplication via `zx_handle_duplicate()`.
        #[allow(clippy::identity_op)]
        const DUPLICATE = 1 << 0;

        /// Allows handle transfer via `zx_channel_write()`.
        const TRANSFER = 1 << 1;

        /// Allows reading of data from containers (channels, sockets, VM objects, etc).
        /// Allows mapping as readable if `Rights::MAP` is also present.
        const READ = 1 << 2;

        /// Allows writing of data to containers (channels, sockets, VM objects, etc).
        /// Allows mapping as writeable if `Rights::MAP` is also present.
        const WRITE = 1 << 3;

        /// Allows mapping as executable if `Rights::MAP` is also present.
        const EXECUTE = 1 << 4;

        /// Allows mapping of a VM object into an address space.
        const MAP = 1 << 5;

        /// Allows property inspection via `zx_object_get_property()`.
        const GET_PROPERTY = 1 << 6;

        /// Allows property modification via `zx_object_set_property()`.
        const SET_PROPERTY = 1 << 7;

        /// Allows enumerating child objects via `zx_object_get_info()` and `zx_object_get_child()`.
        const ENUMERATE = 1 << 8;

        /// Allows termination of task objects via `zx_task_kill()`.
        const DESTROY = 1 << 9;

        /// Allows policy modification via `zx_job_set_policy()`.
        const SET_POLICY = 1 << 10;

        /// Allows policy inspection via `zx_job_get_policy()`.
        const GET_POLICY = 1 << 11;

        /// Allows use of `zx_object_signal()`.
        const SIGNAL = 1 << 12;

        /// Allows use of `zx_object_signal_peer()`.
        const SIGNAL_PEER = 1 << 13;

        /// Allows use of `zx_object_wait_one()`, `zx_object_wait_many()`, and other waiting primitives.
        const WAIT = 1 << 14;

        /// Allows inspection via `zx_object_get_info()`.
        const INSPECT = 1 << 15;

        /// Allows creation of processes, subjobs, etc.
        const MANAGE_JOB = 1 << 16;

        /// Allows creation of threads, etc.
        const MANAGE_PROCESS = 1 << 17;

        /// Allows suspending/resuming threads, etc.
        const MANAGE_THREAD = 1 << 18;

        const APPLY_PROFILE = 1 << 19;
        const SAME_RIGHTS = 1 << 31;

        const BASIC = Self::TRANSFER.bits | Self::DUPLICATE.bits | Self::WAIT.bits | Self::INSPECT.bits;
        const IO = Self::READ.bits | Self::WRITE.bits;
        const PROPERTY = Self::GET_PROPERTY.bits | Self::SET_PROPERTY.bits;
        const POLICY = Self::GET_POLICY.bits | Self::SET_POLICY.bits;

        const DEFAULT_CHANNEL = Self::BASIC.bits & !Self::DUPLICATE.bits | Self::IO.bits | Self::SIGNAL.bits | Self::SIGNAL_PEER.bits;
        const DEFAULT_PROCESS = Self::BASIC.bits | Self::IO.bits | Self::PROPERTY.bits | Self::ENUMERATE.bits | Self::DESTROY.bits
            | Self::SIGNAL.bits | Self::MANAGE_PROCESS.bits | Self::MANAGE_THREAD.bits;
        const DEFAULT_THREAD = Self::BASIC.bits | Self::IO.bits | Self::PROPERTY.bits | Self::DESTROY.bits | Self::SIGNAL.bits | Self::MANAGE_THREAD.bits;
        const DEFAULT_VMO = Self::BASIC.bits | Self::IO.bits | Self::PROPERTY.bits | Self::MAP.bits | Self::SIGNAL.bits;
        const DEFAULT_VMAR = Self::BASIC.bits & !Self::WAIT.bits;
        const DEFAULT_JOB = Self::BASIC.bits | Self::IO.bits | Self::PROPERTY.bits | Self::POLICY.bits | Self::ENUMERATE.bits
            | Self::DESTROY.bits | Self::SIGNAL.bits | Self::MANAGE_JOB.bits | Self::MANAGE_PROCESS.bits | Self::MANAGE_THREAD.bits;
        const DEFAULT_RESOURCE = Self::TRANSFER.bits | Self::DUPLICATE.bits | Self::WRITE.bits | Self::INSPECT.bits;
        const DEFAULT_DEBUGLOG = Self::BASIC.bits | Self::WRITE.bits | Self::SIGNAL.bits;
        const DEFAULT_SUSPEND_TOKEN = Self::TRANSFER.bits | Self::INSPECT.bits;
        const DEFAULT_PORT = (Self::BASIC.bits & !Self::WAIT.bits) | Self::IO.bits;
        const DEFAULT_TIMER = Self::BASIC.bits | Self::WRITE.bits | Self::SIGNAL.bits;
        const DEFAULT_EVENT = Self::BASIC.bits | Self::SIGNAL.bits;
        const DEFAULT_EVENTPAIR = Self::BASIC.bits | Self::SIGNAL.bits | Self::SIGNAL_PEER.bits;
        const DEFAULT_FIFO = Self::BASIC.bits | Self::IO.bits | Self::SIGNAL.bits | Self::SIGNAL_PEER.bits;
        const DEFAULT_SOCKET = Self::BASIC.bits | Self::IO.bits | Self::PROPERTY.bits | Self::SIGNAL.bits | Self::SIGNAL_PEER.bits;
        const DEFAULT_BTI = (Self::BASIC.bits & !Self::WAIT.bits) | Self::IO.bits | Self::MAP.bits;
        const DEFAULT_INTERRUPT = Self::BASIC.bits | Self::IO.bits | Self::SIGNAL.bits;
    }
}

impl TryFrom<u32> for Rights {
    type Error = ZxError;

    fn try_from(x: u32) -> ZxResult<Self> {
        Self::from_bits(x).ok_or(ZxError::INVALID_ARGS)
    }
}
