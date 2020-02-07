#![allow(unsafe_code, dead_code)]

use {
    crate::error::*,
    alloc::string::String,
    alloc::vec::Vec,
    core::fmt::{Debug, Error, Formatter},
    core::marker::PhantomData,
    zircon_object::{ZxError, ZxResult},
};

#[repr(C)]
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

impl<T, P: Policy> From<usize> for UserPtr<T, P> {
    fn from(x: usize) -> Self {
        UserPtr {
            ptr: x as _,
            mark: PhantomData,
        }
    }
}

impl<T, P: Policy> UserPtr<T, P> {
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    pub fn add(&self, count: usize) -> Self {
        UserPtr {
            ptr: unsafe { self.ptr.add(count) },
            mark: PhantomData,
        }
    }

    pub fn as_ptr(&self) -> *mut T {
        self.ptr
    }
}

impl<T, P: Read> UserPtr<T, P> {
    pub fn read(&self) -> ZxResult<T> {
        // TODO: check ptr and return err
        Ok(unsafe { self.ptr.read() })
    }

    pub fn read_array(&self, len: usize) -> ZxResult<Vec<T>> {
        let mut ret = Vec::<T>::with_capacity(len);
        unsafe {
            ret.set_len(len);
            ret.as_mut_ptr().copy_from_nonoverlapping(self.ptr, len);
        }
        Ok(ret)
    }
}

impl<P: Read> UserPtr<u8, P> {
    pub fn read_string(&self, len: usize) -> ZxResult<String> {
        let src = unsafe { core::slice::from_raw_parts(self.ptr, len) };
        let s = core::str::from_utf8(src).map_err(|_| ZxError::INVALID_ARGS)?;
        Ok(String::from(s))
    }

    pub fn read_cstring(&self) -> ZxResult<String> {
        let len = unsafe { (0usize..).find(|&i| *self.ptr.add(i) == 0).unwrap() };
        self.read_string(len)
    }
}

impl<P: Read> UserPtr<UserPtr<u8, P>, P> {
    pub fn read_cstring_array(&self) -> ZxResult<Vec<String>> {
        let len = unsafe {
            (0usize..)
                .find(|&i| self.ptr.add(i).read().is_null())
                .unwrap()
        };
        self.read_array(len)?
            .into_iter()
            .map(|ptr| ptr.read_cstring())
            .collect()
    }
}

impl<T, P: Write> UserPtr<T, P> {
    pub fn write(&mut self, value: T) -> ZxResult<()> {
        unsafe {
            self.ptr.write(value);
        }
        Ok(())
    }

    pub fn write_if_not_null(&mut self, value: T) -> ZxResult<()> {
        if self.ptr.is_null() {
            return Ok(());
        }
        self.write(value)
    }

    pub fn write_array(&mut self, values: &[T]) -> ZxResult<()> {
        unsafe {
            self.ptr
                .copy_from_nonoverlapping(values.as_ptr(), values.len());
        }
        Ok(())
    }
}

impl<P: Write> UserPtr<u8, P> {
    pub fn write_cstring(&mut self, s: &str) -> ZxResult<()> {
        let bytes = s.as_bytes();
        self.write_array(bytes)?;
        unsafe {
            self.ptr.add(bytes.len()).write(0);
        }
        Ok(())
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct IoVec<P: Policy> {
    /// Starting address
    base: UserPtr<u8, P>,
    /// Number of bytes to transfer
    len: usize,
}

/// A valid IoVecs request from user
#[derive(Debug)]
pub struct IoVecs<P: Policy> {
    vec: Vec<IoVec<P>>,
}

impl<P: Policy> IoVecs<P> {
    pub fn new(ptr: UserInPtr<IoVec<P>>, count: usize) -> LxResult<Self> {
        Ok(IoVecs {
            vec: ptr.read_array(count)?,
        })
    }

    pub fn total_len(&self) -> usize {
        self.vec.iter().map(|vec| vec.len).sum()
    }
}

impl<P: Read> IoVecs<P> {
    pub fn read_to_vec(&self) -> LxResult<Vec<u8>> {
        let mut buf = Vec::new();
        for vec in self.vec.iter() {
            buf.extend(vec.base.read_array(vec.len)?);
        }
        Ok(buf)
    }
}

impl<P: Write> IoVecs<P> {
    pub fn write_from_buf(&mut self, mut buf: &[u8]) -> LxResult<usize> {
        let buf_len = buf.len();
        for vec in self.vec.iter_mut() {
            let copy_len = vec.len.min(buf.len());
            if copy_len == 0 {
                continue;
            }
            vec.base.write_array(&buf[..copy_len])?;
            buf = &buf[copy_len..];
        }
        Ok(buf_len - buf.len())
    }
}
