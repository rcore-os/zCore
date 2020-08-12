use crate::types::*;
use crate::error::*;
use crate::object::*;
use alloc::sync::Arc;
use crate::kt::KernelThread;
use crate::sys;
use crate::cap;
use core::marker::PhantomData;
use core::mem::{self, MaybeUninit};
use core::ptr;

struct KipcChannelBacking;
unsafe impl ObjectBacking for KipcChannelBacking {
    fn bits() -> u8 {
        unsafe {
            sys::L4BRIDGE_ENDPOINT_BITS as u8
        }
    }

    unsafe fn retype(untyped: CPtr, out: CPtr) -> KernelResult<()> {
        if sys::locked(|| sys::l4bridge_retype_endpoint(untyped, out)) != 0 {
            Err(KernelError::RetypeFailed)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone)]
pub struct KipcChannel<T: Send + 'static> {
    channel: Arc<Object<KipcChannelBacking>>,
    _phantom: PhantomData<T>,
}

unsafe impl<T: Send + 'static> Sync for KipcChannel<T> {}

impl<T: Send + 'static> KipcChannel<T> {
    pub fn new() -> KernelResult<KipcChannel<T>> {
        Ok(KipcChannel {
            channel: Arc::new(Object::new()?),
            _phantom: PhantomData,
        })
    }

    pub fn recv<'a>(&'a self) -> (T, ReplyHandle<'a, T>) {
        let mut data: usize = 0;
        let mut sender: usize = 0;
        if unsafe {
            sys::l4bridge_kipc_recv(self.channel.object(), &mut data, &mut sender)
        } != 0 {
            // Should never fail.
            panic!("l4bridge_kipc_recv failed");
        }

        (unsafe { ptr::read(data as *const T) }, ReplyHandle { receiver: self })
    }

    pub fn call(&self, msg: T) -> KernelResult<()> {
        let mut result: usize = 0;

        if unsafe {
            sys::l4bridge_kipc_call(self.channel.object(), &msg as *const T as usize, &mut result)
        } != 0 {
            // We are not sure about the current state of `msg` now. Only safe to panic.
            panic!("l4bridge_kipc_call failed");
        }

        // Now the ownership of `msg` is transferred.
        mem::forget(msg);
        if result == 0 {
            Ok(())
        } else {
            Err(KernelError::from_code(result as i32))
        }
    }
}

pub struct ReplyHandle<'a, T: Send + 'static> {
    receiver: &'a KipcChannel<T>,
}

impl<'a, T: Send + 'static> ReplyHandle<'a, T> {
    pub fn send(self, result: usize) {
        unsafe {
            sys::l4bridge_kipc_reply(result);
        }
        mem::forget(self);
    }
    
    pub fn send_recv(&self, result: usize) -> T {
        let mut data: usize = 0;
        let mut sender: usize = 0;
        if unsafe {
            sys::l4bridge_kipc_reply_recv_ts(self.receiver.channel.object(), result, &mut data, &mut sender)
        } != 0 {
            // Should never fail.
            panic!("l4bridge_kipc_reply_recv_ts failed");
        }

        unsafe { ptr::read(data as *const T) }
    }

    pub fn forget(self) {
        mem::forget(self);
    }

    pub fn ok(self) {
        self.send(0);
    }

    pub fn err(self, e: KernelError) {
        self.send(e as i32 as _);
    }

    pub fn save(self) -> KernelResult<SavedReplyHandle> {
        let cap = cap::G.allocate()?;
        if unsafe {
            sys::l4bridge_save_caller(cap)
        } != 0 {
            panic!("ReplyHandle::save: l4bridge_save_caller failed");
        }
        mem::forget(self);
        Ok(SavedReplyHandle {
            cap,
        })
    }
}

impl<'a, T: Send + 'static> Drop for ReplyHandle<'a, T> {
    fn drop(&mut self) {
        unsafe {
            sys::l4bridge_kipc_reply(KernelError::IpcIgnored as i32 as _);
        }
    }
}

pub struct SavedReplyHandle {
    cap: CPtr,
}

impl SavedReplyHandle {
    pub fn send(self, result: usize) {
        unsafe {
            sys::l4bridge_kipc_send_ts(self.cap, result);
            cap::G.release(self.cap);
        }
        mem::forget(self);
    }
}

impl Drop for SavedReplyHandle {
    fn drop(&mut self) {
        unsafe {
            sys::l4bridge_delete_cap_ts(self.cap);
            cap::G.release(self.cap);
        }
    }
}

pub enum KipcLoopOutput<'a, T: Send + 'static> {
    Reply(ReplyHandle<'a, T>, i32),
    NoReply,
}

pub fn kipc_loop<T: Send + 'static, F: for<'a> FnMut(T, ReplyHandle<'a, T>) -> KipcLoopOutput<'a, T>>(ch: &KipcChannel<T>, mut f: F) -> ! {
    let (mut msg, mut reply) = ch.recv();
    loop {
        match f(msg, reply) {
            KipcLoopOutput::Reply(new_reply, x) => {
                msg = new_reply.send_recv(x as _);
                reply = new_reply;
            }
            KipcLoopOutput::NoReply => {
                let (new_msg, new_reply) = ch.recv();
                msg = new_msg;
                reply = new_reply;
            }
        }
    }
}