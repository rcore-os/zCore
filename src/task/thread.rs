use super::process::Process;
use super::*;
use crate::object::KObjectBase;
use alloc::string::String;
use alloc::sync::Arc;

pub struct Thread {
    base: KObjectBase,
    name: String,
    proc: Arc<Process>,
}

impl Thread {
    pub fn create(proc: Arc<Process>, name: &str, options: u32) -> ZxResult<Self> {
        // TODO: options
        // TODO: add thread to proc
        let thread = Thread {
            base: KObjectBase::new(),
            name: String::from(name),
            proc,
        };
        Ok(thread)
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
