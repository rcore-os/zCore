//! Linux userspace ABI invariants (syscall numbers).

#![cfg(test)]

use static_assertions::const_assert_eq;

use crate::consts::SyscallType;

// x86_64 Linux uapi — regressions here break musl/glibc binaries.
#[cfg(target_arch = "x86_64")]
mod x86_64 {
    use super::*;

    const_assert_eq!(0, SyscallType::READ as u32);
    const_assert_eq!(7, SyscallType::POLL as u32);
    const_assert_eq!(41, SyscallType::SOCKET as u32);
    const_assert_eq!(232, SyscallType::EPOLL_WAIT as u32);
    const_assert_eq!(57, SyscallType::FORK as u32);
    const_assert_eq!(59, SyscallType::EXECVE as u32);
    const_assert_eq!(60, SyscallType::EXIT as u32);
    const_assert_eq!(61, SyscallType::WAIT4 as u32);
}
