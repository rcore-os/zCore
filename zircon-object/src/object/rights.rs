use crate::{ZxError, ZxResult};
use core::convert::TryFrom;

bitflags! {
   pub struct Rights: u32 {
        const DUPLICATE = 1 << 0;
        const TRANSFER = 1 << 1;
        const READ = 1 << 2;
        const WRITE = 1 << 3;
        const EXECUTE = 1 << 4;
        const MAP = 1 << 5;
        const GET_PROPERTY = 1 << 6;
        const SET_PROPERTY = 1 << 7;
        const ENUMERATE = 1 << 8;
        const DESTROY = 1 << 9;
        const SET_POLICY = 1 << 10;
        const GET_POLICY = 1 << 11;
        const SIGNAL = 1 << 12;
        const SIGNAL_PEER = 1 << 13;
        const WAIT = 1 << 14;
        const INSPECT = 1 << 15;
        const MANAGE_JOB = 1 << 16;
        const MANAGE_PROCESS = 1 << 17;
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
        const DEFAULT_VMO = Self::BASIC.bits | Self::IO.bits | Self::PROPERTY.bits | Self::MAP.bits | Self::SIGNAL.bits;
        const DEFAULT_VMAR = Self::BASIC.bits & !Self::WAIT.bits;
        const DEFAULT_JOB = Self::BASIC.bits | Self::IO.bits | Self::PROPERTY.bits | Self::POLICY.bits | Self::ENUMERATE.bits
            | Self::DESTROY.bits | Self::SIGNAL.bits | Self::MANAGE_JOB.bits | Self::MANAGE_PROCESS.bits | Self::MANAGE_THREAD.bits;
        const DEFAULT_RESOURCE = Self::TRANSFER.bits | Self::DUPLICATE.bits | Self::WRITE.bits | Self::INSPECT.bits;
   }
}

impl TryFrom<u32> for Rights {
    type Error = ZxError;

    fn try_from(x: u32) -> ZxResult<Self> {
        Self::from_bits(x).ok_or(ZxError::INVALID_ARGS)
    }
}
