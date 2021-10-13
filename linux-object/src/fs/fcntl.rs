//! consts for fcntl
//! currently support x86_64 only
//! copy from fcntl.h (from rCore)
#![allow(dead_code)]

const F_LINUX_SPECIFIC_BASE: usize = 1024;

numeric_enum_macro::numeric_enum! {
    #[repr(usize)]
    #[allow(non_camel_case_types)]
    #[derive(Eq, PartialEq, Debug, Copy, Clone)]
    /// fcntl flags
    pub enum FcntlCmd {
        /// dup
        DUPFD = 0,
        /// get close_on_exec
        GETFD = 1,
        /// set/clear close_on_exec
        SETFD = 2,
        /// get file->f_flags
        GETFL = 3,
        /// set file->f_flags
        SETFL = 4,
        /// Get record locking info.
        GETLK = 5,
        /// Set record locking info (non-blocking).
        SETLK = 6,
        /// Set record locking info (blocking).
        SETLKW = 7,
        /// like F_DUPFD, but additionally set the close-on-exec flag
        DUPFD_CLOEXEC = F_LINUX_SPECIFIC_BASE + 6,
    }
}
