//! Shared user-address validation for syscall buffers.
use kernel_hal::MMUFlags;
use zircon_object::{task::Process, ZxError, ZxResult};

pub(crate) fn validate_user_range(
    proc: &Process,
    addr: usize,
    len: usize,
    access: MMUFlags,
) -> ZxResult {
    proc.vmar()
        .check_user_range(addr, len, access)
        .map_err(|_| ZxError::INVALID_ARGS)
}

pub(crate) fn validate_optional_user_range(
    proc: &Process,
    addr: usize,
    len: usize,
    access: MMUFlags,
) -> ZxResult {
    if addr == 0 {
        Ok(())
    } else {
        validate_user_range(proc, addr, len, access)
    }
}
