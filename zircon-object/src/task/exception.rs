use {
    super::*, crate::ipc::Channel, crate::object::*, alloc::sync::Arc, alloc::vec::Vec,
    core::mem::size_of, core::slice::from_raw_parts, spin::Mutex,
};

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

    pub fn get_channel_handle(&self) -> Option<Arc<Channel>> {
        let inner = self.inner.lock();
        inner.channel_handle.clone()
    }

    pub fn packup_exception(&self, tid: KoID, pid: KoID, excp_type: u32) -> Vec<u8> {
        #[repr(C)]
        pub struct ExceptionInfo {
            tid: KoID,
            pid: KoID,
            type_: u32,
            padding1: [u8; 4],
        }
        let msg = ExceptionInfo {
            tid,
            pid,
            type_: excp_type,
            padding1: [0u8; 4],
        };
        #[allow(unsafe_code)]
        unsafe {
            from_raw_parts(
                (&msg as *const ExceptionInfo) as *const u8,
                size_of::<ExceptionInfo>(),
            )
        }
        .to_vec()
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
