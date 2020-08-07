//! IO Multiplex operations
//!
//! - select4
//! - poll, ppoll
//! - epoll: create, ctl, wait

use super::*;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use linux_object::fs::FileDesc;

impl Syscall<'_> {
    /// Wait for some event on a file descriptor
    pub async fn sys_poll(
        &mut self,
        ufds: UserInOutPtr<PollFd>,
        nfds: usize,
        timeout_msecs: usize,
    ) -> SysResult {
        info!(
            "poll: ufds: {:?}, nfds: {}, timeout_msecs: {:#x}",
            ufds, nfds, timeout_msecs
        );
        let polls = ufds.read_array(nfds).unwrap();
        #[must_use = "future does nothing unless polled/`await`-ed"]
        struct PollFuture<'a> {
            polls: Vec<PollFd>,
            syscall: &'a Syscall<'a>,
        }

        impl<'a> Future for PollFuture<'a> {
            type Output = SysResult;

            #[allow(clippy::drop_ref)]
            fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                use PollEvents as PE;
                let proc = self.syscall.linux_process();
                let mut events = 0;

                // iterate each poll to check whether it is ready
                for poll in self.as_mut().polls.iter_mut() {
                    poll.revents = PE::empty();
                    if let Some(file_like) = proc
                        .inner
                        .lock()
                        .files
                        .get(&FileDesc::from(poll.fd as usize))
                    {
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
                drop(proc);

                // some event happens, so evoke the process
                if events > 0 {
                    return Poll::Ready(Ok(events));
                }
                Poll::Pending
            }
        }
        let future = PollFuture {
            polls,
            syscall: self,
        };
        future.await
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
