use crate::types::*;
use crate::error::*;
use crate::object::*;
use alloc::sync::Arc;
use crate::kt::KernelThread;
use crate::sys;
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

        (unsafe { ptr::read(data as *const T) }, ReplyHandle { _receiver: self })
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
    _receiver: &'a KipcChannel<T>,
}

impl<'a, T: Send + 'static> ReplyHandle<'a, T> {
    fn send(self, result: usize) {
        unsafe {
            sys::l4bridge_kipc_reply(result);
        }
        mem::forget(self);
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
}

impl<'a, T: Send + 'static> Drop for ReplyHandle<'a, T> {
    fn drop(&mut self) {
        unsafe {
            sys::l4bridge_kipc_reply(KernelError::IpcIgnored as i32 as _);
        }
    }
}
