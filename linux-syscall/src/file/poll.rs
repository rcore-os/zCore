//! IO Multiplex operations
//!
//! - select, pselect
//! - poll, ppoll

use super::*;
use alloc::boxed::Box;
use alloc::vec::Vec;
use bitvec::prelude::{BitVec, Lsb0};
use core::future::Future;
use core::mem::size_of;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;
use kernel_hal::timer;
use linux_object::fs::FileDesc;
use linux_object::time::*;

impl Syscall<'_> {
    /// Wait for some event on a file descriptor
    pub async fn sys_poll(
        &mut self,
        mut ufds: UserInOutPtr<PollFd>,
        nfds: usize,
        timeout_msecs: isize,
    ) -> SysResult {
        let mut polls = ufds.read_array(nfds)?;
        info!(
            "poll: ufds: {:?}, nfds: {:?}, timeout_msecs: {:#x}",
            polls, nfds, timeout_msecs
        );
        #[must_use = "future does nothing unless polled/`await`-ed"]
        struct PollFuture<'a> {
            polls: &'a mut Vec<PollFd>,
            timeout_msecs: isize,
            begin_time_ms: usize,
            syscall: &'a Syscall<'a>,
        }

        let begin_time_ms = TimeVal::now().to_msec();

        impl<'a> Future for PollFuture<'a> {
            type Output = SysResult;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                use PollEvents as PE;
                let proc = self.syscall.linux_process();
                let mut events = 0;

                // iterate each poll to check whether it is ready
                for poll in self.as_mut().polls.iter_mut() {
                    poll.revents = PE::empty();
                    if let Ok(file_like) = proc.get_file_like(poll.fd) {
                        let mut fut = Box::pin(file_like.async_poll());
                        let status = match fut.as_mut().poll(cx) {
                            Poll::Ready(Ok(ret)) => ret,
                            Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                            Poll::Pending => continue,
                        };
                        if status.error {
                            poll.revents |= PE::HUP;
                            events += 1;
                        }
                        if status.read && poll.events.contains(PE::IN) {
                            poll.revents |= PE::IN;
                            events += 1;
                        }
                        if status.write && poll.events.contains(PE::OUT) {
                            poll.revents |= PE::OUT;
                            events += 1;
                        }
                    } else {
                        poll.revents |= PE::ERR;
                        events += 1;
                    }
                }
                // some event happens, so evoke the process
                if events > 0 {
                    return Poll::Ready(Ok(events));
                }

                match self.timeout_msecs {
                    // no timeout, return now;
                    0 => return Poll::Ready(Ok(0)),
                    1.. => {
                        let current_time_ms = TimeVal::now().to_msec();
                        let deadline = self.begin_time_ms + self.timeout_msecs as usize;
                        if current_time_ms >= deadline {
                            return Poll::Ready(Ok(0));
                        } else {
                            let waker = cx.waker().clone();
                            timer::timer_set(
                                Duration::from_millis(deadline as u64),
                                Box::new(move |_| waker.wake()),
                            );
                        }
                    }
                    _ => {}
                }

                Poll::Pending
            }
        }
        let future = PollFuture {
            polls: &mut polls,
            timeout_msecs,
            begin_time_ms,
            syscall: self,
        };
        let result = future.await;
        ufds.write_array(&polls)?;
        result
    }

    /// Wait for some event on a file descriptor
    ///
    /// ppoll() allows an application to safely wait until either a file descriptor becomes ready or until a signal is caught
    pub async fn sys_ppoll(
        &mut self,
        ufds: UserInOutPtr<PollFd>,
        nfds: usize,
        timeout: UserInPtr<TimeSpec>,
    ) -> SysResult {
        let timeout_msecs = if timeout.is_null() {
            -1
        } else {
            let timeout = timeout.read().unwrap();
            timeout.to_msec() as isize
        };

        self.sys_poll(ufds, nfds, timeout_msecs).await
    }

    /// similar to select, but have sigmask argument
    pub async fn sys_pselect6(
        &mut self,
        nfds: usize,
        read: UserInOutPtr<u32>,
        write: UserInOutPtr<u32>,
        err: UserInOutPtr<u32>,
        timeout: UserInPtr<TimeVal>,
        _sigset: usize,
    ) -> SysResult {
        self.sys_select(nfds, read, write, err, timeout).await
    }

    /// allow a program to monitor multiple file descriptors,
    /// waiting until one or more of the file descriptors become "ready" for some class of I/O operation.
    ///
    /// A file descriptor is considered ready if it is possible to perform the corresponding I/O operation (e.g., read) without blocking.
    pub async fn sys_select(
        &mut self,
        nfds: usize,
        read: UserInOutPtr<u32>,
        write: UserInOutPtr<u32>,
        err: UserInOutPtr<u32>,
        timeout: UserInPtr<TimeVal>,
    ) -> SysResult {
        info!(
            "select: nfds: {}, read: {:?}, write: {:?}, err: {:?}, timeout: {:?}",
            nfds, read, write, err, timeout
        );
        if nfds as u64 == 0 {
            return Ok(0);
        }
        let mut read_fds = FdSet::new(read, nfds)?;
        let mut write_fds = FdSet::new(write, nfds)?;
        let mut err_fds = FdSet::new(err, nfds)?;

        let timeout_msecs = if !timeout.is_null() {
            let timeout = timeout.read()?;
            timeout.to_msec() as isize
        } else {
            // infinity
            -1
        };
        let begin_time_ms = TimeVal::now().to_msec();

        #[must_use = "future does nothing unless polled/`await`-ed"]
        struct SelectFuture<'a> {
            read_fds: &'a mut FdSet,
            write_fds: &'a mut FdSet,
            err_fds: &'a mut FdSet,
            timeout_msecs: isize,
            begin_time_ms: usize,
            syscall: &'a Syscall<'a>,
        }

        impl<'a> Future for SelectFuture<'a> {
            type Output = SysResult;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                let files = self.syscall.linux_process().get_files()?;

                let mut events = 0;
                for (&fd, file_like) in files.iter() {
                    if !self.err_fds.contains(fd)
                        && !self.read_fds.contains(fd)
                        && !self.write_fds.contains(fd)
                    {
                        continue;
                    }
                    let mut fut = Box::pin(file_like.async_poll());
                    let status = match fut.as_mut().poll(cx) {
                        Poll::Ready(Ok(ret)) => ret,
                        Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                        Poll::Pending => continue,
                    };
                    if status.error && self.err_fds.contains(fd) {
                        self.err_fds.set(fd);
                        events += 1;
                    }
                    if status.read && self.read_fds.contains(fd) {
                        self.read_fds.set(fd);
                        events += 1;
                    }
                    if status.write && self.write_fds.contains(fd) {
                        self.write_fds.set(fd);
                        events += 1;
                    }
                }

                // some event happens, so evoke the process
                if events > 0 {
                    return Poll::Ready(Ok(events));
                }

                match self.timeout_msecs {
                    // no timeout, return now;
                    0 => return Poll::Ready(Ok(0)),
                    1.. => {
                        let current_time_ms = TimeVal::now().to_msec();
                        let deadline = self.begin_time_ms + self.timeout_msecs as usize;
                        if current_time_ms >= deadline {
                            return Poll::Ready(Ok(0));
                        } else {
                            let waker = cx.waker().clone();
                            timer::timer_set(
                                Duration::from_millis(deadline as u64),
                                Box::new(move |_| waker.wake()),
                            );
                        }
                    }
                    _ => {}
                }
                Poll::Pending
            }
        }
        let future = SelectFuture {
            read_fds: &mut read_fds,
            write_fds: &mut write_fds,
            err_fds: &mut err_fds,
            timeout_msecs,
            begin_time_ms,
            syscall: self,
        };
        future.await
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct PollFd {
    fd: FileDesc,
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

/// fd size per item
const FD_PER_ITEM: usize = 8 * size_of::<u32>();
/// max Fdset size
const MAX_FDSET_SIZE: usize = 1024 / FD_PER_ITEM;

/// FdSet data struct for select
struct FdSet {
    /// input addr, for update Fdset use
    addr: UserInOutPtr<u32>,
    /// FdSet bit buffer
    origin: BitVec<Lsb0, u32>,
}

impl FdSet {
    /// Initialize a `FdSet` from pointer and number of fds
    /// Check if the array is large enough
    fn new(mut addr: UserInOutPtr<u32>, nfds: usize) -> Result<FdSet, LxError> {
        if addr.is_null() {
            Ok(FdSet {
                addr,
                origin: BitVec::new(),
            })
        } else {
            let len = (nfds + FD_PER_ITEM - 1) / FD_PER_ITEM;
            if len > MAX_FDSET_SIZE {
                return Err(LxError::EINVAL);
            }
            let slice = addr.read_array(len)?;

            // save the fdset, and clear it
            let origin = BitVec::from_vec(slice);
            let mut vec0 = Vec::<u32>::new();
            vec0.resize(len, 0);
            addr.write_array(&vec0)?;
            Ok(FdSet { addr, origin })
        }
    }

    /// Try to set fd in `FdSet`
    /// Return true when `FdSet` is valid, and false when `FdSet` is bad (i.e. null pointer)
    /// Fd should be less than nfds
    fn set(&mut self, fd: FileDesc) -> bool {
        let fd: usize = fd.into();
        if self.origin.is_empty() {
            return false;
        }
        self.origin.set(fd, true);
        let vec: Vec<u32> = self.origin.clone().into();
        self.addr.write_array(&vec).is_ok()
    }

    /// Check to see whether `fd` is in original `FdSet`
    /// Fd should be less than nfds
    fn contains(&self, fd: FileDesc) -> bool {
        let fd: usize = fd.into();
        if fd < self.origin.len() {
            self.origin[fd]
        } else {
            false
        }
    }
}
