//! Read/write user space pointer.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{Debug, Formatter};
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};

use crate::VirtAddr;

/// Wapper of raw pointer from user space.
#[repr(C)]
pub struct UserPtr<T, P: Policy> {
    ptr: *mut T,
    mark: PhantomData<P>,
}

/// Marker of user pointer policy trait.
pub trait Policy {}

/// Marks a pointer used to read.
pub trait Read: Policy {}

/// Marks a pointer used to write.
pub trait Write: Policy {}

/// Type argument for user pointer used to read.
pub struct In;

/// Type argument for user pointer used to write.
pub struct Out;

/// Type argument for user pointer used to both read and write.
pub struct InOut;

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

/// The error type which is returned from user pointer.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidUtf8,
    InvalidPointer,
    BufferTooSmall,
    InvalidLength,
    InvalidVectorAddress,
}

type Result<T> = core::result::Result<T, Error>;

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
    /// Checks if `size` is enough to save a value of `T`,
    /// then constructs a user pointer from its value `addr`.
    pub fn from_addr_size(addr: usize, size: usize) -> Result<Self> {
        if size >= core::mem::size_of::<T>() {
            Ok(Self::from(addr))
        } else {
            Err(Error::BufferTooSmall)
        }
    }

    /// Returns true if the pointer is null.
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    /// Calculates the offset from a pointer.
    /// `count` is in units of `T`;
    /// e.g., a count of 3 represents a pointer offset of `3 * size_of::<T>()` bytes.
    pub fn add(&self, count: usize) -> Self {
        UserPtr {
            ptr: unsafe { self.ptr.add(count) },
            mark: PhantomData,
        }
    }

    /// Returns the raw pointer.
    pub fn as_addr(&self) -> VirtAddr {
        self.ptr as _
    }

    /// Checks avaliability of the user pointer.
    ///
    /// Returns [`Ok(())`] if it is neither null nor unaligned,
    pub fn check(&self) -> Result<()> {
        if !self.ptr.is_null() && (self.ptr as usize) % core::mem::align_of::<T>() == 0 {
            Ok(())
        } else {
            Err(Error::InvalidPointer)
        }
    }
}

impl<T, P: Read> UserPtr<T, P> {
    /// Always returns a shared reference to the value wrapped in [`Ok`].
    pub fn as_ref(&self) -> Result<&'static T> {
        Ok(unsafe { &*self.ptr })
    }

    /// Reads the value from self without moving it.
    /// This leaves the memory in self unchanged.
    pub fn read(&self) -> Result<T> {
        self.check()?;
        Ok(unsafe { self.ptr.read() })
    }

    /// Same as [`read`](Self::read),
    /// but returns [`None`] when pointer is null.
    pub fn read_if_not_null(&self) -> Result<Option<T>> {
        if !self.ptr.is_null() {
            Ok(Some(self.read()?))
        } else {
            Ok(None)
        }
    }

    /// Copies elements into a new [`Vec`].
    ///
    /// The `len` argument is the number of **elements**, not the number of bytes.
    pub fn read_array(&self, len: usize) -> Result<Vec<T>> {
        if len == 0 {
            return Ok(Vec::default());
        }
        self.check()?;
        let mut ret = Vec::<T>::with_capacity(len);
        unsafe {
            ret.set_len(len);
            ret.as_mut_ptr().copy_from_nonoverlapping(self.ptr, len);
        }
        Ok(ret)
    }
}

impl<P: Read> UserPtr<u8, P> {
    /// Copies chars into a new [`String`].
    ///
    /// The `len` argument is the number of **bytes**, not the number of chars.
    pub fn read_string(&self, len: usize) -> Result<String> {
        self.check()?;
        let src = unsafe { core::slice::from_raw_parts(self.ptr, len) };
        let s = core::str::from_utf8(src).map_err(|_| Error::InvalidUtf8)?;
        Ok(String::from(s))
    }

    /// Copies an zero-terminated string of c style to a new [`String`].
    pub fn read_cstring(&self) -> Result<String> {
        self.check()?;
        let len = unsafe { (0usize..).find(|&i| *self.ptr.add(i) == 0).unwrap() };
        self.read_string(len)
    }
}

impl<P: Read> UserPtr<UserPtr<u8, P>, P> {
    /// Copies a group of zero-terminated string into [`String`]s,
    /// and collect them into a [`Vec`].
    pub fn read_cstring_array(&self) -> Result<Vec<String>> {
        self.check()?;
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
    /// Overwrites a memory location with the given value
    /// **without** reading or dropping the old value.
    pub fn write(&mut self, value: T) -> Result<()> {
        self.check()?;
        unsafe { self.ptr.write(value) };
        Ok(())
    }

    /// Same as [`write`](Self::write),
    /// but does nothing and returns [`Ok`] when pointer is null.
    pub fn write_if_not_null(&mut self, value: T) -> Result<()> {
        if !self.ptr.is_null() {
            self.write(value)
        } else {
            Ok(())
        }
    }

    /// Copies `values.len() * size_of<T>` bytes from `values` to `self`.
    /// The source and destination may not overlap.
    pub fn write_array(&mut self, values: &[T]) -> Result<()> {
        if !values.is_empty() {
            self.check()?;
            unsafe {
                self.ptr
                    .copy_from_nonoverlapping(values.as_ptr(), values.len())
            };
        }
        Ok(())
    }
}

impl<P: Write> UserPtr<u8, P> {
    /// Copies `s` to `self`, then write a `'\0'` for c style string.
    pub fn write_cstring(&mut self, s: &str) -> Result<()> {
        let bytes = s.as_bytes();
        self.write_array(bytes)?;
        unsafe { self.ptr.add(bytes.len()).write(0) };
        Ok(())
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
        self.as_mut_slice().map(|s| &*s)
    }

    pub fn as_mut_slice(&self) -> Result<&mut [u8]> {
        if !self.ptr.is_null() {
            Ok(unsafe { core::slice::from_raw_parts_mut(self.ptr.ptr, self.len) })
        } else {
            Err(Error::InvalidVectorAddress)
        }
    }
}
