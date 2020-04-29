#![allow(dead_code)]

use {
    super::*, crate::ipc::Channel, crate::object::*, alloc::sync::Arc, alloc::vec::Vec,
    core::mem::size_of, spin::Mutex,
};

/// Kernel-owned exception channel endpoint.
pub struct Exceptionate {
    type_: ExceptionChannelType,
    inner: Mutex<ExceptionateInner>,
}

struct ExceptionateInner {
    channel: Option<Arc<Channel>>,
    thread_rights: Rights,
    process_rights: Rights,
}

impl Exceptionate {
    pub fn new(type_: ExceptionChannelType) -> Arc<Self> {
        Arc::new(Exceptionate {
            type_,
            inner: Mutex::new(ExceptionateInner {
                channel: None,
                thread_rights: Rights::empty(),
                process_rights: Rights::empty(),
            }),
        })
    }

    pub fn set_channel(&self, channel: Arc<Channel>) {
        let mut inner = self.inner.lock();
        inner.channel.replace(channel);
    }

    pub fn get_channel(&self) -> Option<Arc<Channel>> {
        let inner = self.inner.lock();
        inner.channel.clone()
    }
}

#[repr(C)]
pub struct ExceptionInfo {
    pub tid: KoID,
    pub pid: KoID,
    pub type_: ExceptionChannelType,
    pub padding: u32,
}

#[repr(u32)]
pub enum ExceptionChannelType {
    None = 0,
    Debugger = 1,
    Thread = 2,
    Process = 3,
    Job = 4,
    JobDebugger = 5,
}

impl ExceptionInfo {
    #[allow(unsafe_code)]
    pub fn pack(&self) -> Vec<u8> {
        let buf: [u8; size_of::<ExceptionInfo>()] = unsafe { core::mem::transmute_copy(self) };
        Vec::from(buf)
    }
}
