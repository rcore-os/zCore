use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{Debug, Formatter};
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use crate::user::UserProcess;
use core::mem::MaybeUninit;
use core::sync::atomic::AtomicI32;

#[repr(C)]
pub struct UserPtr<T, P: Policy> {
    ptr: *mut T,
    mark: PhantomData<P>,
}

impl<T, P: Policy> Clone for UserPtr<T, P> {
    fn clone(&self) -> Self {
        Self {
            ptr: self.ptr,
            mark: PhantomData,
        }
    }
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

type Result<T> = core::result::Result<T, Error>;

/// The error type which is returned from user pointer.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidUtf8,
    InvalidPointer,
    BufferTooSmall,
    InvalidLength,
    InvalidVectorAddress,
}

impl<T, P: Policy> Debug for UserPtr<T, P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self.ptr)
    }
}

// FIXME: this is a workaround for `clear_child_tid`.
unsafe impl<T, P: Policy> Send for UserPtr<T, P> {}
unsafe impl<T, P: Policy> Sync for UserPtr<T, P> {}

impl<T, P: Policy> From<usize> for UserPtr<T, P> {
    fn from(x: usize) -> Self {
        UserPtr {
            ptr: x as _,
            mark: PhantomData,
        }
    }
}

impl<T, P: Policy> UserPtr<T, P> {
    pub fn from_addr_size(addr: usize, size: usize) -> Result<Self> {
        if size < core::mem::size_of::<T>() {
            return Err(Error::BufferTooSmall);
        }
        Ok(Self::from(addr))
    }

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

    pub fn check(&self) -> Result<()> {
        if self.ptr.is_null() {
            return Err(Error::InvalidPointer);
        }
        if (self.ptr as usize) % core::mem::align_of::<T>() != 0 {
            return Err(Error::InvalidPointer);
        }
        Ok(())
    }
}

impl<P: Read> UserPtr<AtomicI32, P> {
    pub fn read_atomic(&self) -> Result<i32> {
        UserProcess::with_current(|p| unsafe {
            p.read_memory_typed_atomic(super::IDMAP, self.ptr as usize)
        }).map_err(|_| Error::InvalidPointer)
    }

    pub fn write_atomic(&self, value: i32) -> Result<()> {
        UserProcess::with_current(|p| unsafe {
            p.write_memory_typed_atomic(super::IDMAP, self.ptr as usize, value)
        }).map_err(|_| Error::InvalidPointer)
    }
}

impl<T, P: Read> UserPtr<T, P> {
    pub fn read(&self) -> Result<T> {
        UserProcess::with_current(|p| unsafe {
            p.read_memory_typed(super::IDMAP, self.ptr as usize)
        }).map_err(|_| Error::InvalidPointer)
    }

    pub fn read_if_not_null(&self) -> Result<Option<T>> {
        if self.ptr.is_null() {
            return Ok(None);
        }
        let value = self.read()?;
        Ok(Some(value))
    }

    pub fn read_array(&self, len: usize) -> Result<Vec<T>> {
        if len == 0 {
            return Ok(Vec::default());
        }
        self.check()?;
        let mut ret = Vec::<T>::with_capacity(len);
        unsafe {
            ret.set_len(len);
            UserProcess::with_current(|p| unsafe {
                let out_slice = core::slice::from_raw_parts_mut(
                    ret.as_mut_ptr() as *mut u8,
                    len * core::mem::size_of::<T>(),
                );
                p.read_memory(super::IDMAP, self.ptr as usize, out_slice)
            }).map_err(|_| Error::InvalidPointer)?;
        }
        Ok(ret)
    }
}

impl<P: Read> UserPtr<u8, P> {
    pub fn read_string(&self, len: usize) -> Result<String> {
        self.check()?;
        let v: Vec<u8> = self.read_array(len)?;
        Ok(String::from_utf8(v).map_err(|_| Error::InvalidUtf8)?)
    }

    pub fn read_cstring(&self) -> Result<String> {
        unimplemented!("read_cstring");
    }
}

impl<P: Read> UserPtr<UserPtr<u8, P>, P> {
    pub fn read_cstring_array(&self) -> Result<Vec<String>> {
        unimplemented!("read_cstring_array");
    }
}

impl<T, P: Write> UserPtr<T, P> {
    pub fn write(&mut self, value: T) -> Result<()> {
        UserProcess::with_current(|p| unsafe {
            p.write_memory_typed(super::IDMAP, self.ptr as usize, value)
        }).map_err(|_| Error::InvalidPointer)
    }

    pub fn write_if_not_null(&mut self, value: T) -> Result<()> {
        if self.ptr.is_null() {
            return Ok(());
        }
        self.write(value)
    }

    pub fn write_array(&mut self, values: &[T]) -> Result<()> {
        if values.len() == 0 {
            return Ok(());
        }
        UserProcess::with_current(|p| unsafe {
            let slice = core::slice::from_raw_parts(
                values.as_ptr() as *const u8,
                values.len() * core::mem::size_of::<T>(),
            );
            p.write_memory(super::IDMAP, self.ptr as usize, slice)
        }).map_err(|_| Error::InvalidPointer)
    }
}

impl<P: Write> UserPtr<u8, P> {
    pub fn write_cstring(&mut self, s: &str) -> Result<()> {
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        self.write_array(&bytes)
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct IoVec<P: Policy> {
    /// Starting address
    ptr: UserPtr<u8, P>,
    /// Number of bytes to transfer
    len: usize,
}

pub type IoVecIn = IoVec<In>;
pub type IoVecOut = IoVec<Out>;

/// A valid IoVecs request from user
#[derive(Debug)]
pub struct IoVecs<P: Policy> {
    vec: Vec<IoVec<P>>,
}

impl<P: Policy> UserInPtr<IoVec<P>> {
    pub fn read_iovecs(&self, count: usize) -> Result<IoVecs<P>> {
        if self.ptr.is_null() {
            return Err(Error::InvalidPointer);
        }
        let vec = self.read_array(count)?;
        // The sum of length should not overflow.
        let mut total_count = 0usize;
        for io_vec in vec.iter() {
            let (result, overflow) = total_count.overflowing_add(io_vec.len());
            if overflow {
                return Err(Error::InvalidLength);
            }
            total_count = result;
        }
        Ok(IoVecs { vec })
    }
}

impl<P: Policy> IoVecs<P> {
    pub fn total_len(&self) -> usize {
        self.vec.iter().map(|vec| vec.len).sum()
    }
}

impl<P: Read> IoVecs<P> {
    pub fn read_to_vec(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        for vec in self.vec.iter() {
            buf.extend(vec.ptr.read_array(vec.len)?);
        }
        Ok(buf)
    }
}

impl<P: Write> IoVecs<P> {
    pub fn write_from_buf(&mut self, mut buf: &[u8]) -> Result<usize> {
        let buf_len = buf.len();
        for vec in self.vec.iter_mut() {
            let copy_len = vec.len.min(buf.len());
            if copy_len == 0 {
                continue;
            }
            vec.ptr.write_array(&buf[..copy_len])?;
            buf = &buf[copy_len..];
        }
        Ok(buf_len - buf.len())
    }
}

impl<P: Policy> Deref for IoVecs<P> {
    type Target = [IoVec<P>];

    fn deref(&self) -> &Self::Target {
        self.vec.as_slice()
    }
}

impl<P: Write> DerefMut for IoVecs<P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.vec.as_mut_slice()
    }
}

impl<P: Policy> IoVec<P> {
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn check(&self) -> Result<()> {
        self.ptr.check()
    }

    pub fn as_slice(&self) -> Result<&[u8]> {
        if self.ptr.is_null() {
            return Err(Error::InvalidVectorAddress);
        }
        let slice = unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len) };
        Ok(slice)
    }
}

impl<P: Write> IoVec<P> {
    pub fn as_mut_slice(&mut self) -> Result<&mut [u8]> {
        if self.ptr.is_null() {
            return Err(Error::InvalidVectorAddress);
        }
        let slice = unsafe { core::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) };
        Ok(slice)
    }
}
