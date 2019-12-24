use super::process::Process;
use super::*;
use crate::object::*;
use alloc::string::String;
use alloc::sync::Arc;

pub struct Thread {
    base: KObjectBase,
    name: String,
    proc: Arc<Process>,
}

impl_kobject!(Thread);

impl Thread {
    pub fn create(proc: &Arc<Process>, name: &str, _options: u32) -> ZxResult<Arc<Self>> {
        // TODO: options
        let thread = Arc::new(Thread {
            base: KObjectBase::new(),
            name: String::from(name),
            proc: proc.clone(),
        });
        proc.add_thread(thread.clone());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create() {
        let proc = Process::create(&job::ROOT_JOB, "proc", 0).expect("failed to create process");
        let thread = Thread::create(&proc, "thread", 0).expect("failed to create thread");
    }
}
