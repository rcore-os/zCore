#![allow(unsafe_code)]

use crate::ZxResult;
use core::fmt::{Debug, Error, Formatter};
use core::marker::PhantomData;

pub struct UserPtr<T, P: Policy> {
    ptr: *mut T,
    mark: PhantomData<P>,
}

pub trait Policy {}
pub trait Read: Policy {}
pub trait Write: Policy {}
pub enum In {}
pub enum Out {}
pub enum InOut {}

impl Policy for In {}
impl Policy for Out {}
impl Policy for InOut {}
impl Read for In {}
impl Write for Out {}
impl Read for InOut {}
impl Write for InOut {}

pub type UserInPtr<T> = UserPtr<T, In>;
pub type UserOutPtr<T> = UserPtr<T, Out>;
pub type UserInOutPtr<T> = UserPtr<T, InOut>;

impl<T, P: Policy> Debug for UserPtr<T, P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{:?}", self.ptr)
    }
}

impl<T, P: Read> UserPtr<T, P> {
    pub fn read(&self) -> ZxResult<T> {
        // TODO: check ptr and return err
        Ok(unsafe { self.ptr.read() })
    }
}

impl<T: Copy, P: Write> UserPtr<T, P> {
    pub fn write(&self, value: T) -> ZxResult<()> {
        unsafe {
            self.ptr.write(value);
        }
        Ok(())
    }

    pub fn write_if_not_null(&self, value: T) -> ZxResult<()> {
        if self.ptr.is_null() {
            return Ok(());
        }
        self.write(value)
    }

    pub fn write_array(&self, values: &[T]) -> ZxResult<()> {
        unsafe {
            core::slice::from_raw_parts_mut(self.ptr, values.len()).copy_from_slice(values);
        }
        Ok(())
    }
}
