//! IO Multiplex operations
//!
//! - select4
//! - poll, ppoll
//! - epoll: create, ctl, wait

use super::*;

impl Syscall<'_> {
    /// Wait for some event on a file descriptor
    pub fn sys_poll(
        &mut self,
        _ufds: UserInOutPtr<PollFd>,
        _nfds: usize,
        _timeout_msecs: usize,
    ) -> SysResult {
        // TODO
        Ok(0)
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct PollFd {
    fd: u32,
    events: PollEvents,
    revents: PollEvents,
}

bitflags! {
    pub struct PollEvents: u16 {
        /// There is data to read.
        const IN = 0x0001;
        /// Writing is now possible.
        const OUT = 0x0004;
        /// Error condition (return only)
        const ERR = 0x0008;
        /// Hang up (return only)
        const HUP = 0x0010;
        /// Invalid request: fd not open (return only)
        const INVAL = 0x0020;
    }
}
