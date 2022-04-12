// 来自用户空间的裸指针
//! Raw pointer from user land.

use crate::VirtAddr;
use alloc::{string::String, vec::Vec};
use core::{
    fmt::{Debug, Formatter},
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

// 来自用户空间的裸指针
/// Raw pointer from user land.
#[repr(transparent)]
pub struct UserPtr<T, P: Policy>(*mut T, PhantomData<P>);

// 标识用户指针功能的基特征。
/// Base trait for Markers of user pointer policy.
pub trait Policy {}

// 标记一个用于输入的指针。
/// Marks a pointer used to read.
pub trait Read: Policy {}

// 标记一个用于输出的指针。
/// Marks a pointer used to write.
pub trait Write: Policy {}

// 输入指针的类型参数。
/// Type argument for user pointer used to read.
pub struct In;

// 输出指针的类型参数。
/// Type argument for user pointer used to write.
pub struct Out;

// 既用于输入有用于输出的指针的类型参数。
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

// 用户指针操作的异常类型。
/// The error type which is returned from user pointer operation.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidUtf8,
    InvalidPointer,
    BufferTooSmall,
    InvalidLength,
    InvalidVectorAddress,
}

// 本模块用到的只是用户指针操作结果的类型。
type Result<T> = core::result::Result<T, Error>;

impl<T, P: Policy> Debug for UserPtr<T, P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        // 打印用户指针就是打印裸指针
        write!(f, "{:?}", self.0)
    }
}

// FIXME: this is a workaround for `clear_child_tid`.
unsafe impl<T, P: Policy> Send for UserPtr<T, P> {}
unsafe impl<T, P: Policy> Sync for UserPtr<T, P> {}

impl<T, P: Policy> From<usize> for UserPtr<T, P> {
    fn from(ptr: usize) -> Self {
        UserPtr(ptr as _, PhantomData)
    }
}

impl<T, P: Policy> UserPtr<T, P> {
    // 检查 `size` 是否足够放下一个 `T` 的值，
    // 并从 `addr` 构造一个用户指针。
    /// Checks if `size` is enough to save a value of `T`,
    /// then constructs a user pointer from its value `addr`.
    pub fn from_addr_size(addr: usize, size: usize) -> Result<Self> {
        if size >= core::mem::size_of::<T>() {
            Ok(Self::from(addr))
        } else {
            Err(Error::BufferTooSmall)
        }
    }

    // 如果指针为空，返回 `true`。
    /// Returns `true` if the pointer is null.
    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }

    // 偏移指针。
    // `count` 表示 `T` 的数量；
    // 例如，`count` 为 3 表示将指针移动 `3 * size_of::<T>()` 个字节。
    /// Calculates the offset from a pointer.
    /// `count` is in units of `T`;
    /// e.g., a `count` of 3 represents a pointer offset of `3 * size_of::<T>()` bytes.
    pub fn add(&self, count: usize) -> Self {
        Self(unsafe { self.0.add(count) }, PhantomData)
    }

    // 返回指针对应的虚地址。
    /// Returns the virtual address represented by the pointer.
    pub fn as_addr(&self) -> VirtAddr {
        self.0 as _
    }

    // 检查用户指针是否合法。
    //
    // 如果指针非空且对齐则返回 `OK(())`。
    /// Checks avaliability of the user pointer.
    ///
    /// Returns [`Ok(())`] if it is neither null nor unaligned.
    pub fn check(&self) -> Result<()> {
        if !self.0.is_null() && (self.0 as usize) % core::mem::align_of::<T>() == 0 {
            Ok(())
        } else {
            Err(Error::InvalidPointer)
        }
    }
}

impl<T, P: Read> UserPtr<T, P> {
    // 取出指针值的引用（不要用于小于 8 字节的类型）。
    /// Converts to reference.
    #[allow(clippy::should_implement_trait)]
    pub fn as_ref(&self) -> &'static T {
        unsafe { &*self.0 }
    }

    // 读取但不移动指针所指的值（通过逐字节拷贝，但不需要 `Copy` 特征）。
    // 指针所指的值保持不变。
    /// Reads the value from `self` without moving it.
    /// This leaves the memory in self unchanged.
    pub fn read(&self) -> Result<T> {
        self.check()?;
        Ok(unsafe { self.0.read() })
    }

    // 和读取一样，
    // 但若指针为空，返回 `None`。
    /// Same as [`read`](Self::read),
    /// but returns [`None`] when pointer is null.
    pub fn read_if_not_null(&self) -> Result<Option<T>> {
        if !self.0.is_null() {
            Ok(Some(self.read()?))
        } else {
            Ok(None)
        }
    }

    // 构造一个从指针指向开始，长度为 `len` 的切片。
    /// Forms a slice from a user pointer and a `len`.
    pub fn as_slice(&self, len: usize) -> Result<&'static [T]> {
        if len == 0 {
            Ok(&[])
        } else {
            self.check()?;
            Ok(unsafe { core::slice::from_raw_parts(self.0, len) })
        }
    }

    // 拷贝对象来构造一个 `Vec`。
    //
    // `len` 是成员的数量，而不是字节数。
    /// Copies elements into a new [`Vec`].
    ///
    /// The `len` argument is the number of **elements**, not the number of bytes.
    pub fn read_array(&self, len: usize) -> Result<Vec<T>> {
        if len == 0 {
            Ok(Vec::default())
        } else {
            self.check()?;
            let mut ret = Vec::<T>::with_capacity(len);
            unsafe {
                ret.set_len(len);
                ret.as_mut_ptr().copy_from_nonoverlapping(self.0, len);
            }
            Ok(ret)
        }
    }
}

impl<P: Read> UserPtr<u8, P> {
    // 构造一个从指针指向开始，长度为 `len` 的字符切片。
    /// Forms an utf-8 string slice from a user pointer and a `len`.
    pub fn as_str(&self, len: usize) -> Result<&'static str> {
        core::str::from_utf8(self.as_slice(len)?).map_err(|_| Error::InvalidUtf8)
    }

    // 从一个 C 风格的零结尾字符串构造一个字符切片。
    /// Forms a zero-terminated string slice from a user pointer to a c style string.
    pub fn as_c_str(&self) -> Result<&'static str> {
        self.as_str(unsafe { (0usize..).find(|&i| *self.0.add(i) == 0).unwrap() })
    }
}

impl<P: 'static + Read> UserPtr<UserPtr<u8, P>, P> {
    // 拷贝一组 C 风格的零结尾字符串到 `String`，
    // 并收集到一个 `Vec` 中。
    /// Copies a group of zero-terminated string into [`String`]s,
    /// and collect them into a [`Vec`].
    pub fn read_cstring_array(&self) -> Result<Vec<String>> {
        self.check()?;
        let mut result = Vec::new();
        let mut pptr = self.0;
        loop {
            let sptr = unsafe { pptr.read() };
            if sptr.is_null() {
                break;
            }
            result.push(sptr.as_c_str()?.into());
            pptr = unsafe { pptr.add(1) };
        }
        Ok(result)
    }
}

impl<T, P: Write> UserPtr<T, P> {
    // 用指定的值覆盖指针位置。
    // 旧的值直接被覆盖，不会调用释放逻辑。
    /// Overwrites a memory location with the given `value`
    /// **without** reading or dropping the old value.
    pub fn write(&mut self, value: T) -> Result<()> {
        self.check()?;
        unsafe { self.0.write(value) };
        Ok(())
    }

    // 类似于写，
    // 但指针为空时返回 `Ok(())`。
    /// Same as [`write`](Self::write),
    /// but does nothing and returns [`Ok`] when pointer is null.
    pub fn write_if_not_null(&mut self, value: T) -> Result<()> {
        if !self.0.is_null() {
            self.write(value)
        } else {
            Ok(())
        }
    }

    // 写入 `values.len() * size_of<T>` 字节到指针位置。
    // 写入的区间与目标区间不可重叠。
    /// Copies `values.len() * size_of<T>` bytes from `values` to `self`.
    /// The source and destination may not overlap.
    pub fn write_array(&mut self, values: &[T]) -> Result<()> {
        if !values.is_empty() {
            self.check()?;
            unsafe {
                self.0
                    .copy_from_nonoverlapping(values.as_ptr(), values.len())
            };
        }
        Ok(())
    }
}

impl<P: Write> UserPtr<u8, P> {
    // 拷贝指定字符串到目标位置并写入一个 `\0` 来模拟 C 风格零结尾字符串。
    /// Copies `s` to `self`, then write a `'\0'` for c style string.
    pub fn write_cstring(&mut self, s: &str) -> Result<()> {
        let bytes = s.as_bytes();
        self.write_array(bytes)?;
        unsafe { self.0.add(bytes.len()).write(0) };
        Ok(())
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct IoVec<P: 'static + Policy> {
    /// Starting address
    ptr: UserPtr<u8, P>,
    /// Number of bytes to transfer
    len: usize,
}

pub type IoVecIn = IoVec<In>;
pub type IoVecOut = IoVec<Out>;

/// A valid IoVecs request from user
#[derive(Debug)]
pub struct IoVecs<P: 'static + Policy> {
    vec: Vec<IoVec<P>>,
}

impl<P: Policy> UserInPtr<IoVec<P>> {
    pub fn read_iovecs(&self, count: usize) -> Result<IoVecs<P>> {
        if self.0.is_null() {
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
            buf.extend_from_slice(vec.ptr.as_slice(vec.len)?);
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
            Ok(unsafe { core::slice::from_raw_parts_mut(self.ptr.0, self.len) })
        } else {
            Err(Error::InvalidVectorAddress)
        }
    }
}
