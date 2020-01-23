use {
    super::process::Process, super::*, crate::object::*, alloc::string::String, alloc::sync::Arc,
    spin::Mutex,
};

pub struct Thread {
    base: KObjectBase,
    #[allow(dead_code)]
    name: String,
    pub proc: Arc<Process>,
    inner: Mutex<ThreadInner>,
}

impl_kobject!(Thread);

#[derive(Default)]
struct ThreadInner {
    hal_thread: Option<crate::hal::Thread>,
}

impl Thread {
    pub fn create(proc: &Arc<Process>, name: &str, _options: u32) -> ZxResult<Arc<Self>> {
        // TODO: options
        let thread = Arc::new(Thread {
            base: KObjectBase::new(),
            name: String::from(name),
            proc: proc.clone(),
            inner: Mutex::new(ThreadInner::default()),
        });
        proc.add_thread(thread.clone());
        Ok(thread)
    }

    /// Get current `Thread` object.
    pub fn current() -> Arc<Self> {
        crate::hal::Thread::tls()
    }

    pub fn start(
        self: &Arc<Self>,
        entry: usize,
        stack: usize,
        arg1: usize,
        arg2: usize,
    ) -> ZxResult<()> {
        let hal_thread = crate::hal::Thread::spawn(entry, stack, arg1, arg2, self.clone());
        let mut inner = self.inner.lock();
        if inner.hal_thread.is_some() {
            return Err(ZxError::BAD_STATE);
        }
        inner.hal_thread = Some(hal_thread);
        Ok(())
    }

    pub fn exit(&self) -> ZxResult<()> {
        self.inner
            .lock()
            .hal_thread
            .as_mut()
            .ok_or(ZxError::BAD_STATE)?
            .exit();
        Ok(())
    }

    pub fn read_state(&self, _kind: ThreadStateKind) -> ZxResult<ThreadState> {
        unimplemented!()
    }

    pub fn write_state(&self, _state: &ThreadState) -> ZxResult<()> {
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
    use super::job::Job;
    use super::*;
    use std::sync::atomic::*;

    #[test]
    fn create() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc", 0).expect("failed to create process");
        let _thread = Thread::create(&proc, "thread", 0).expect("failed to create thread");
    }

    #[test]
    #[allow(unsafe_code)]
    fn start() {
        let root_job = Job::root();
        let proc = Process::create(&root_job, "proc", 0).expect("failed to create process");
        let thread = Thread::create(&proc, "thread", 0).expect("failed to create thread");
        let thread1 = Thread::create(&proc, "thread1", 0).expect("failed to create thread");

        // allocate stack for new thread
        static mut STACK: [u8; 0x1000] = [0u8; 0x1000];
        let stack_top = unsafe { STACK.as_ptr() } as usize + 0x1000;

        // global variable for validation
        static ARG1: AtomicUsize = AtomicUsize::new(0);
        static ARG2: AtomicUsize = AtomicUsize::new(0);

        // function for new thread
        extern "C" fn entry(arg1: usize, arg2: usize) -> ! {
            // switch back to kernel
            crate::hal::swap_fs();
            ARG1.store(arg1, Ordering::SeqCst);
            ARG2.store(arg2, Ordering::SeqCst);
            loop {
                std::thread::park();
            }
        }

        // start a new thread
        let handle = Handle::new(proc.clone(), Rights::DEFAULT_PROCESS);
        proc.start(&thread, entry as usize, stack_top, handle.clone(), 2)
            .expect("failed to start thread");

        // wait 100ms for the new thread to exit
        std::thread::sleep(core::time::Duration::from_millis(100));

        // validate the thread have started and received correct arguments
        assert_eq!(ARG1.load(Ordering::SeqCst), 0);
        assert_eq!(ARG2.load(Ordering::SeqCst), 2);

        // start again should fail
        assert_eq!(
            proc.start(&thread, entry as usize, stack_top, handle.clone(), 2),
            Err(ZxError::BAD_STATE)
        );

        // start another thread should fail
        assert_eq!(
            proc.start(&thread1, entry as usize, stack_top, handle.clone(), 2),
            Err(ZxError::BAD_STATE)
        );
    }
}
