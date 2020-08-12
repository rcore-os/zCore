//! consts for fctnl
//! currently support x86_64 only
//! copy from fcntl.h (from rCore)
#![allow(dead_code)]

use bitflags::bitflags;

const F_LINUX_SPECIFIC_BASE: usize = 1024;

bitflags! {
    /// fcntl flags
    pub struct FcntlFlags: usize {
        /// dup
        const F_DUPFD = 0;
        /// get close_on_exec
        const F_GETFD = 1;
        /// set/clear close_on_exec
        const F_SETFD = 2;
        /// get file->f_flags
        const F_GETFL = 3;
        /// set file->f_flags
        const F_SETFL = 4;
        /// Get record locking info.
        const F_GETLK = 5;
        /// Set record locking info (non-blocking).
        const F_SETLK = 6;
        /// Set record locking info (blocking).
        const F_SETLKW = 7;
        /// closed during a successful execve
        const FD_CLOEXEC = 1;
        /// like F_DUPFD, but additionally set the close-on-exec flag
        const F_DUPFD_CLOEXEC = F_LINUX_SPECIFIC_BASE + 6;
    }
}

bitflags! {
    /// file operate flags
    pub struct FileFlags: usize {
        /// not blocking
        const O_NONBLOCK = 0o4000;
        /// move the flag bit to the end of the file before each write
        const O_APPEND = 0o2000;
        /// set close_on_exec
        const O_CLOEXEC = 0o2000000;
    }
}
