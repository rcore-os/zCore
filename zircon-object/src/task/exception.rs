use {super::*, crate::ipc::Channel, crate::object::*, alloc::sync::Arc, spin::Mutex};

#[allow(dead_code)]
pub struct Exceptionate {
    type_: ZxExceptionChannelType,
    inner: Mutex<ExceptionateInner>,
}

#[allow(dead_code)]
struct ExceptionateInner {
    channel_handle: Option<Arc<Channel>>,
    thread_rights: Rights,
    process_rights: Rights,
}

impl Exceptionate {
    pub fn new(type_: ZxExceptionChannelType) -> Arc<Self> {
        Arc::new(Exceptionate {
            type_,
            inner: Mutex::new(ExceptionateInner {
                channel_handle: None,
                thread_rights: Rights::empty(),
                process_rights: Rights::empty(),
            }),
        })
    }

    pub fn set_channel(&self, channel: Arc<Channel>) {
        let mut inner = self.inner.lock();
        inner.channel_handle.replace(channel);
    }
}

pub enum ZxExceptionChannelType {
    None,
    Debugger,
    Thread,
    Process,
    Job,
    JobDebugger,
}
