use super::job::Job;
use super::thread::Thread;
use super::*;
use crate::memory::vmar::VMAR;
use crate::object::handle::Handle;

pub struct Process {}

impl Process {
    pub fn create(job: &mut Job, name: &str, options: u32) -> ZxResult<(Self, VMAR)> {
        unimplemented!()
    }

    pub fn start(
        &mut self,
        thread: &Thread,
        entry: usize,
        stack: usize,
        arg1: Handle,
        arg2: usize,
    ) {
        unimplemented!()
    }

    pub fn exit(&mut self, retcode: usize) {
        unimplemented!()
    }
}
