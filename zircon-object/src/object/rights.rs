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

        /// Not used.
        const APPLY_PROFILE = 1 << 19;

        /// Used to duplicate a handle with the same rights.
        const SAME_RIGHTS = 1 << 31;


        /// TRANSFER | DUPLICATE | WAIT | INSPECT
        const BASIC = Self::TRANSFER.bits | Self::DUPLICATE.bits | Self::WAIT.bits | Self::INSPECT.bits;

        /// READ ｜ WRITE
        const IO = Self::READ.bits | Self::WRITE.bits;

        /// GET_PROPERTY ｜ SET_PROPERTY
        const PROPERTY = Self::GET_PROPERTY.bits | Self::SET_PROPERTY.bits;

        /// GET_POLICY ｜ SET_POLICY
        const POLICY = Self::GET_POLICY.bits | Self::SET_POLICY.bits;

        /// BASIC & !Self::DUPLICATE | IO | SIGNAL | SIGNAL_PEER
        const DEFAULT_CHANNEL = Self::BASIC.bits & !Self::DUPLICATE.bits | Self::IO.bits | Self::SIGNAL.bits | Self::SIGNAL_PEER.bits;

        /// BASIC | IO | PROPERTY | ENUMERATE | DESTROY | SIGNAL | MANAGE_PROCESS | MANAGE_THREAD
        const DEFAULT_PROCESS = Self::BASIC.bits | Self::IO.bits | Self::PROPERTY.bits | Self::ENUMERATE.bits | Self::DESTROY.bits
            | Self::SIGNAL.bits | Self::MANAGE_PROCESS.bits | Self::MANAGE_THREAD.bits;

        /// BASIC | IO | PROPERTY | DESTROY | SIGNAL | MANAGE_THREAD
        const DEFAULT_THREAD = Self::BASIC.bits | Self::IO.bits | Self::PROPERTY.bits | Self::DESTROY.bits | Self::SIGNAL.bits | Self::MANAGE_THREAD.bits;

        /// BASIC | IO | PROPERTY | MAP | SIGNAL
        const DEFAULT_VMO = Self::BASIC.bits | Self::IO.bits | Self::PROPERTY.bits | Self::MAP.bits | Self::SIGNAL.bits;

        /// BASIC | WAIT
        const DEFAULT_VMAR = Self::BASIC.bits & !Self::WAIT.bits;

        /// BASIC | IO | PROPERTY | POLICY | ENUMERATE | DESTROY | SIGNAL | MANAGE_JOB | MANAGE_PROCESS | MANAGE_THREAD
        const DEFAULT_JOB = Self::BASIC.bits | Self::IO.bits | Self::PROPERTY.bits | Self::POLICY.bits | Self::ENUMERATE.bits
            | Self::DESTROY.bits | Self::SIGNAL.bits | Self::MANAGE_JOB.bits | Self::MANAGE_PROCESS.bits | Self::MANAGE_THREAD.bits;

        /// TRANSFER | DUPLICATE | WRITE | INSPECT
        const DEFAULT_RESOURCE = Self::TRANSFER.bits | Self::DUPLICATE.bits | Self::WRITE.bits | Self::INSPECT.bits;

        /// BASIC | WRITE | SIGNAL
        const DEFAULT_DEBUGLOG = Self::BASIC.bits | Self::WRITE.bits | Self::SIGNAL.bits;

        /// TRANSFER | INSPECT
        const DEFAULT_SUSPEND_TOKEN = Self::TRANSFER.bits | Self::INSPECT.bits;

        /// (BASIC & !WAIT) | IO
        const DEFAULT_PORT = (Self::BASIC.bits & !Self::WAIT.bits) | Self::IO.bits;

        /// BASIC | WRITE | SIGNAL
        const DEFAULT_TIMER = Self::BASIC.bits | Self::WRITE.bits | Self::SIGNAL.bits;

        /// BASIC | SIGNAL
        const DEFAULT_EVENT = Self::BASIC.bits | Self::SIGNAL.bits;

        /// BASIC | SIGNAL ｜ SIGNAL_PEER
        const DEFAULT_EVENTPAIR = Self::BASIC.bits | Self::SIGNAL.bits | Self::SIGNAL_PEER.bits;

        /// BASIC | IO | SIGNAL | SIGNAL_PEER
        const DEFAULT_FIFO = Self::BASIC.bits | Self::IO.bits | Self::SIGNAL.bits | Self::SIGNAL_PEER.bits;

        /// BASIC | IO | PROPERTY | SIGNAL | SIGNAL_PEER
        const DEFAULT_SOCKET = Self::BASIC.bits | Self::IO.bits | Self::PROPERTY.bits | Self::SIGNAL.bits | Self::SIGNAL_PEER.bits;

        /// BASIC | PROPERTY | SIGNAL
        const DEFAULT_STREAM = Self::BASIC.bits | Self::PROPERTY.bits | Self::SIGNAL.bits;

        /// (BASIC & !WAIT) | IO | MAP
        const DEFAULT_BTI = (Self::BASIC.bits & !Self::WAIT.bits) | Self::IO.bits | Self::MAP.bits;

        /// BASIC | IO | SIGNAL
        const DEFAULT_INTERRUPT = Self::BASIC.bits | Self::IO.bits | Self::SIGNAL.bits;

        /// BASIC | IO
        const DEFAULT_DEVICE = Self::BASIC.bits | Self::IO.bits;

        /// BASIC | IO | SIGNAL
        const DEFAULT_PCI_INTERRUPT = Self::BASIC.bits | Self::IO.bits | Self::SIGNAL.bits;

        /// TRANSFER | PROPERTY | INSPECT
        const DEFAULT_EXCEPTION = Self::TRANSFER.bits | Self::PROPERTY.bits | Self::INSPECT.bits;

        /// TRANSFER | DUPLICATE | WRITE | INSPECT | MANAGE_PROCESS
        const DEFAULT_GUEST = Self::TRANSFER.bits | Self::DUPLICATE.bits | Self::WRITE.bits | Self::INSPECT.bits | Self::MANAGE_PROCESS.bits;

        /// BASIC | IO | EXECUTE | SIGNAL
        const DEFAULT_VCPU = Self::BASIC.bits | Self::IO.bits | Self::EXECUTE.bits | Self::SIGNAL.bits;
    }
}

impl TryFrom<u32> for Rights {
    type Error = ZxError;

    fn try_from(x: u32) -> ZxResult<Self> {
        Self::from_bits(x).ok_or(ZxError::INVALID_ARGS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_try_from() {
        assert_eq!(Err(ZxError::INVALID_ARGS), Rights::try_from(0xffff_ffff));
        assert_eq!(Ok(Rights::SAME_RIGHTS), Rights::try_from(1 << 31));
    }
}
