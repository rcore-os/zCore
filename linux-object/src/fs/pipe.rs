//! Implement INode for Pipe

use alloc::{collections::vec_deque::VecDeque, sync::Arc};
use core::{any::Any, cmp::min};
use rcore_fs::vfs::*;
use spin::Mutex;

#[derive(Clone, PartialEq)]
#[allow(dead_code)]
pub enum PipeEnd {
    Read,
    Write,
}

pub struct PipeData {
    /// pipe buffer
    buf: VecDeque<u8>,
    /// number of pipe ends
    end_cnt: i32,
}

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
    }
}

#[allow(dead_code)]
impl Pipe {
    /// Create a pair of INode: (read, write)
    pub fn create_pair() -> (Pipe, Pipe) {
        let inner = PipeData {
            buf: VecDeque::new(),
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

    fn can_read(&self) -> bool {
        if let PipeEnd::Read = self.direction {
            // true
            let data = self.data.lock();
            !data.buf.is_empty() || data.end_cnt < 2 // other end closed
        } else {
            false
        }
    }

    fn can_write(&self) -> bool {
        if let PipeEnd::Write = self.direction {
            self.data.lock().end_cnt == 2
        } else {
            false
        }
    }
}

impl INode for Pipe {
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
                Ok(len)
            }
        } else {
            Ok(0)
        }
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        if let PipeEnd::Write = self.direction {
            let mut data = self.data.lock();
            for c in buf {
                data.buf.push_back(*c);
            }
            Ok(buf.len())
        } else {
            Ok(0)
        }
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: self.can_read(),
            write: self.can_write(),
            error: false,
        })
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}
