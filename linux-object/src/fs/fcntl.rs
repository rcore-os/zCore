//! consts for fctnl
//! currently support x86_64 only
//! copy from fcntl.h (from rCore)
#![allow(dead_code)]

/// dup
pub const F_DUPFD: usize = 0;
/// get close_on_exec
pub const F_GETFD: usize = 1;
/// set/clear close_on_exec
pub const F_SETFD: usize = 2;
/// get file->f_flags
pub const F_GETFL: usize = 3;
/// set file->f_flags
pub const F_SETFL: usize = 4;
/// Get record locking info.
pub const F_GETLK: usize = 5;
/// Set record locking info (non-blocking).
pub const F_SETLK: usize = 6;
/// Set record locking info (blocking).
pub const F_SETLKW: usize = 7;

/// SPECIFIC BASE for other
const F_LINUX_SPECIFIC_BASE: usize = 1024;

/// closed during a successful execve
pub const FD_CLOEXEC: usize = 1;
/// like F_DUPFD, but additionally set the close-on-exec flag
pub const F_DUPFD_CLOEXEC: usize = F_LINUX_SPECIFIC_BASE + 6;

/// not blocking
pub const O_NONBLOCK: usize = 0o4000;
/// move the flag bit to the end of the file before each write
pub const O_APPEND: usize = 0o2000;
/// set close_on_exec
pub const O_CLOEXEC: usize = 0o2000000;

/// Do not follow symbolic links.
pub const AT_SYMLINK_NOFOLLOW: usize = 0x100;
