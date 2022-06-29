//! Implement INode for Pipe
#![deny(missing_docs)]

use crate::{sync::Event, sync::EventBus};
use alloc::{boxed::Box, collections::vec_deque::VecDeque, sync::Arc};
use core::{any::Any, cmp::min};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use lock::Mutex;
use rcore_fs::vfs::*;

#[derive(Clone, PartialEq)]
#[allow(dead_code)]
/// Pipe end specify
pub enum PipeEnd {
    /// read end
    Read,
    /// write end
    Write,
}

/// Pipe inner data
pub struct PipeData {
    /// pipe buffer
    buf: VecDeque<u8>,
    /// event bus for pipe
    eventbus: EventBus,
    /// number of pipe ends
    end_cnt: i32,
}

/// pipe struct
#[derive(Clone)]
pub struct Pipe {
    data: Arc<Mutex<PipeData>>,
    direction: PipeEnd,
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // pipe end closed
        let mut data = self.data.lock();
        data.end_cnt -= 1;
        data.eventbus.set(Event::CLOSED);
    }
}

#[allow(dead_code)]
impl Pipe {
    /// Create a pair of INode: (read, write)
    pub fn create_pair() -> (Pipe, Pipe) {
        let inner = PipeData {
            buf: VecDeque::new(),
            eventbus: EventBus::default(),
            end_cnt: 2, // one read, one write
        };
        let data = Arc::new(Mutex::new(inner));
        (
            Pipe {
                data: data.clone(),
                direction: PipeEnd::Read,
            },
            Pipe {
                data,
                direction: PipeEnd::Write,
            },
        )
    }
    /// whether the pipe struct is readable
    fn can_read(&self) -> bool {
        if let PipeEnd::Read = self.direction {
            // true
            let data = self.data.lock();
            !data.buf.is_empty() || data.end_cnt < 2 // other end closed
        } else {
            false
        }
    }

    /// whether the pipe struct is writeable
    fn can_write(&self) -> bool {
        if let PipeEnd::Write = self.direction {
            self.data.lock().end_cnt == 2
        } else {
            false
        }
    }
}

impl INode for Pipe {
    /// read from pipe
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if let PipeEnd::Read = self.direction {
            let mut data = self.data.lock();
            if data.buf.is_empty() && data.end_cnt == 2 {
                Err(FsError::Again)
            } else {
                let len = min(buf.len(), data.buf.len());
                for item in buf.iter_mut().take(len) {
                    *item = data.buf.pop_front().unwrap();
                }
                if data.buf.is_empty() {
                    data.eventbus.clear(Event::READABLE);
                }
                Ok(len)
            }
        } else {
            Ok(0)
        }
    }

    /// write to pipe
    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        if let PipeEnd::Write = self.direction {
            let mut data = self.data.lock();
            for c in buf {
                data.buf.push_back(*c);
            }
            data.eventbus.set(Event::READABLE);
            Ok(buf.len())
        } else {
            Ok(0)
        }
    }

    /// monitoring events and determine whether the pipe is readable or writeable
    /// if the write end is not close and the buffer is empty, the read end will be block
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: self.can_read(),
            write: self.can_write(),
            error: false,
        })
    }

    fn async_poll<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<PollStatus>> + Send + Sync + 'a>> {
        #[must_use = "future does nothing unless polled/`await`-ed"]
        struct PipeFuture<'a> {
            pipe: &'a Pipe,
        }

        impl<'a> Future for PipeFuture<'a> {
            type Output = Result<PollStatus>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                if self.pipe.can_read() || self.pipe.can_write() {
                    return Poll::Ready(self.pipe.poll());
                }
                let waker = cx.waker().clone();
                let mut data = self.pipe.data.lock();
                data.eventbus.subscribe(Box::new({
                    move |_| {
                        waker.wake_by_ref();
                        true
                    }
                }));
                Poll::Pending
            }
        }

        Box::pin(PipeFuture { pipe: self })
    }

    /// return the any ref
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}
