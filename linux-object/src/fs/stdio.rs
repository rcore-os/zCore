//! Implement INode for Stdin & Stdout
#![allow(unsafe_code)]

// use super::ioctl::*;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::any::Any;
use lazy_static::lazy_static;
use rcore_fs::vfs::*;
use spin::Mutex;

lazy_static! {
    pub static ref STDIN: Arc<Stdin> = Default::default();
    pub static ref STDOUT: Arc<Stdout> = Default::default();
}

#[derive(Default)]
pub struct Stdin {
    buf: Mutex<VecDeque<char>>,
    // TODO: add signal
}

impl Stdin {
    pub fn push(&self, c: char) {
        self.buf.lock().push_back(c);
        // self.pushed.notify_one();
    }
    pub fn pop(&self) -> char {
        loop {
            let mut buf_lock = self.buf.lock();
            if let Some(c) = buf_lock.pop_front() {
                return c;
            } else {
                // self.pushed.wait(buf_lock);
            }
        }
    }
    pub fn can_read(&self) -> bool {
        self.buf.lock().len() > 0
    }
}

#[derive(Default)]
pub struct Stdout;

impl INode for Stdin {
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        if self.can_read() {
            buf[0] = self.pop() as u8;
            Ok(1)
        } else {
            let mut buffer = [0; 255];
            let len = kernel_hal::serial_read(&mut buffer);
            for c in &buffer[..len] {
                self.push((*c).into());
            }
            Err(FsError::Again)
        }
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        unimplemented!()
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: self.can_read(),
            write: false,
            error: false,
        })
    }
    /*
    fn io_control(&self, cmd: u32, data: usize) -> Result<()> {
        match cmd as usize {
            TCGETS | TIOCGWINSZ | TIOCSPGRP => {
                // pretend to be tty
                Ok(())
            }
            TIOCGPGRP => {
                // pretend to be have a tty process group
                // TODO: verify pointer
                unsafe { *(data as *mut u32) = 0 };
                Ok(())
            }
            _ => Err(FsError::NotSupported),
        }
    }
    */
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}

impl INode for Stdout {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        unimplemented!()
    }
    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        // we do not care the utf-8 things, we just want to print it!
        let s = unsafe { core::str::from_utf8_unchecked(buf) };
        kernel_hal::serial_write(s);
        Ok(buf.len())
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: false,
            write: true,
            error: false,
        })
    }
    /*
    fn io_control(&self, cmd: u32, data: usize) -> Result<()> {
        match cmd as usize {
            TCGETS | TIOCGWINSZ | TIOCSPGRP => {
                // pretend to be tty
                Ok(())
            }
            TIOCGPGRP => {
                // pretend to be have a tty process group
                // TODO: verify pointer
                unsafe { *(data as *mut u32) = 0 };
                Ok(())
            }
            _ => Err(FsError::NotSupported),
        }
    }
    */
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}
