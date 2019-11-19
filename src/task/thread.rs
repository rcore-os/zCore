use super::process::Process;
use super::*;

pub struct Thread {}

impl Thread {
    pub fn create(proc: &mut Process, name: &str, options: u32) -> ZxResult<Self> {
        unimplemented!()
    }

    pub fn start(&mut self, entry: usize, stack: usize, arg1: usize, arg2: usize) {
        unimplemented!()
    }

    pub fn exit(&mut self) {
        unimplemented!()
    }

    pub fn read_state(&self, kind: ThreadStateKind) -> ZxResult<ThreadState> {
        unimplemented!()
    }

    pub fn write_state(&self, state: &ThreadState) -> ZxResult<()> {
        unimplemented!()
    }
}

#[repr(u32)]
#[derive(Debug, Copy, Clone)]
pub enum ThreadStateKind {
    General = 0,
    FloatPoint = 1,
    Vector = 2,
    Debug = 4,
    SingleStep = 5,
    FS = 6,
    GS = 7,
}

#[derive(Debug)]
pub enum ThreadState {}
