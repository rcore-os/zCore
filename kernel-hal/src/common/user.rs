// 来自用户空间的裸指针
//! Raw pointer from user land.

use crate::VirtAddr;
use alloc::{string::String, vec::Vec};
use core::{
    fmt::{Debug, Formatter},
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

/// DEBUG: escanea los bytes que el kernel está a punto de copiar a memoria de
/// usuario buscando el patrón LE `00 80 ff ff` (= la mitad alta `0xffff8000` de
/// un puntero physmap del kernel). Si aparece, el kernel está FILTRANDO un
/// puntero de kernel a usuario — la causa de que apk acabe con `rbp =
/// 0xffff8000_xxxxxxxx`. Escaneo acotado a 4 KiB para no frenar copias grandes.
///
/// Gated behind the `uleak-scan` feature: the scan is a byte-stride pass over
/// up to 4 KiB on the return path of EVERY data-returning syscall (read,
/// readv, getdents64, fstat, recvfrom, ...), which is far too expensive to
/// leave in the default build now that the original leak is fixed. Re-enable
/// the feature to chase a regression.
#[cfg(all(not(feature = "libos"), feature = "uleak-scan"))]
fn dbg_scan_physmap_leak(bytes: &[u8], dst: usize, who: &str) {
    let n = bytes.len().min(4096);
    let mut i = 0;
    while i + 4 <= n {
        if bytes[i] == 0x00 && bytes[i + 1] == 0x80 && bytes[i + 2] == 0xff && bytes[i + 3] == 0xff
        {
            warn!(
                "[uleak] physmap-high 0xffff8000 -> usuario dst={:#x} off={} via={} len={}",
                dst,
                i,
                who,
                bytes.len()
            );
            return;
        }
        i += 1;
    }
}

// 来自用户空间的裸指针
/// Raw pointer from user land.
#[repr(transparent)]
#[derive(Copy, Clone)]
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

/// First address above the user half. Canonical x86_64 splits at
/// `0x0000_8000_0000_0000`, and Sv39/Sv48/aarch64 user ranges all sit below it,
/// so one constant covers every bare-metal target.
#[cfg(not(feature = "libos"))]
const USER_MAX: usize = 0x0000_8000_0000_0000;

/// Whether `[addr, addr + bytes)` lies entirely in the user half.
///
/// The kernel is mapped into EVERY address space, so a pointer that arrived
/// from userspace naming a kernel address is not a fault waiting to happen —
/// it resolves, and the copy lands in kernel memory. `check()` previously
/// tested only null and alignment, which made every syscall out-pointer an
/// arbitrary kernel-memory write and every in-pointer an arbitrary kernel-memory
/// read. Linux rejects these with EFAULT via `access_ok()`; so do we.
///
/// `libos` builds run in a host process where "user" addresses are ordinary
/// host addresses, so the bound does not apply there.
#[inline]
fn in_user_half(addr: usize, bytes: usize) -> bool {
    #[cfg(feature = "libos")]
    {
        let _ = (addr, bytes);
        true
    }
    #[cfg(not(feature = "libos"))]
    {
        match addr.checked_add(bytes) {
            Some(end) => end <= USER_MAX,
            None => false,
        }
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
    /// Returns [`Ok(())`] if it is neither null nor unaligned, and lies in the
    /// user half of the address space.
    pub fn check(&self) -> Result<()> {
        self.check_len(1)
    }

    /// [`check`](Self::check) for a run of `count` elements starting here, so a
    /// slice that STARTS in the user half cannot run off its top end into the
    /// kernel.
    pub fn check_len(&self, count: usize) -> Result<()> {
        let bytes = count
            .checked_mul(core::mem::size_of::<T>())
            .ok_or(Error::InvalidLength)?;
        if !self.0.is_null()
            && (self.0 as usize).is_multiple_of(core::mem::align_of::<T>())
            && in_user_half(self.0 as usize, bytes)
        {
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
            self.check_len(len)?;
            Ok(unsafe { core::slice::from_raw_parts(self.0, len) })
        }
    }

    // 拷贝对象来构造一个 `Vec`。
    //
    // `len` 是成员的数量，而不是字节数。
    /// Copies elements into a new [`Vec`].
    ///
    /// The `len` argument is the number of **elements**, not the number of bytes.
    #[allow(clippy::uninit_vec)]
    pub fn read_array(&self, len: usize) -> Result<Vec<T>> {
        if len == 0 {
            Ok(Vec::default())
        } else {
            self.check_len(len)?;
            // The total number of bytes to copy must not overflow `usize`,
            // otherwise the allocation would be smaller than `set_len` claims
            // and the following copy would write out of bounds.
            len.checked_mul(core::mem::size_of::<T>())
                .ok_or(Error::InvalidLength)?;
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
    ///
    /// The scan for the terminating `'\0'` is bounded by [`MAX_C_STR_LEN`] so a
    /// malicious or buggy user pointer that is not null-terminated cannot make
    /// the kernel walk an unbounded amount of memory (and the previous
    /// `unwrap()` could panic the kernel).
    pub fn as_c_str(&self) -> Result<&'static str> {
        self.check()?;
        let len = (0..MAX_C_STR_LEN)
            .find(|&i| unsafe { *self.0.add(i) == 0 })
            .ok_or(Error::InvalidLength)?;
        self.as_str(len)
    }
}

/// Upper bound for the length of a C string read from user space.
const MAX_C_STR_LEN: usize = 4 * 1024 * 1024;

/// Upper bound for the number of entries in a user-supplied pointer array
/// (e.g. `argv`/`envp`), to bound kernel work and allocations.
const MAX_C_STR_ARRAY_LEN: usize = 1 << 20;

impl<P: 'static + Read> UserPtr<UserPtr<u8, P>, P> {
    // 拷贝一组 C 风格的零结尾字符串到 `String`，
    // 并收集到一个 `Vec` 中。
    /// Copies a group of zero-terminated string into [`String`]s,
    /// and collect them into a [`Vec`].
    pub fn read_cstring_array(&self) -> Result<Vec<String>> {
        self.check()?;
        let mut result = Vec::new();
        let mut pptr = self.0;
        for _ in 0..MAX_C_STR_ARRAY_LEN {
            let sptr = unsafe { pptr.read() };
            if sptr.is_null() {
                return Ok(result);
            }
            result.push(sptr.as_c_str()?.into());
            pptr = unsafe { pptr.add(1) };
        }
        Err(Error::InvalidLength)
    }
}

impl<T, P: Write> UserPtr<T, P> {
    // 用指定的值覆盖指针位置。
    // 旧的值直接被覆盖，不会调用释放逻辑。
    /// Overwrites a memory location with the given `value`
    /// **without** reading or dropping the old value.
    pub fn write(&mut self, value: T) -> Result<()> {
        self.check()?;
        #[cfg(all(not(feature = "libos"), feature = "uleak-scan"))]
        dbg_scan_physmap_leak(
            unsafe {
                core::slice::from_raw_parts(
                    &value as *const T as *const u8,
                    core::mem::size_of::<T>(),
                )
            },
            self.0 as usize,
            "write",
        );
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
            self.check_len(values.len())?;
            #[cfg(all(not(feature = "libos"), feature = "uleak-scan"))]
            dbg_scan_physmap_leak(
                unsafe {
                    core::slice::from_raw_parts(
                        values.as_ptr() as *const u8,
                        core::mem::size_of_val(values),
                    )
                },
                self.0 as usize,
                "write_array",
            );
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
        // +1: the NUL below is written past the array, so it must be bounded too.
        self.check_len(bytes.len() + 1)?;
        self.write_array(bytes)?;
        unsafe { self.0.add(bytes.len()).write(0) };
        Ok(())
    }
}

#[repr(C)]
pub struct IoVec<P: Policy> {
    /// Starting address
    ptr: UserPtr<u8, P>,
    /// Number of bytes to transfer
    len: usize,
}

impl<P: Policy> core::fmt::Debug for IoVec<P> {
    fn fmt(&self, f: &mut Formatter) -> core::fmt::Result {
        write!(f, "IoVec ptr:{:?} len :{:?}", self.ptr.0, self.len)
    }
}
pub type IoVecIn = IoVec<In>;
pub type IoVecOut = IoVec<Out>;
pub type IoVecsOut = IoVecs<Out>;

/// A valid IoVecs request from user
pub struct IoVecs<P: 'static + Policy> {
    vec: Vec<IoVec<P>>,
}

impl<P: Policy> Debug for IoVecs<P> {
    fn fmt(&self, f: &mut Formatter) -> core::fmt::Result {
        write!(f, "IoVec len :{:?}", self.vec.len())
    }
}

impl IoVecs<Out> {
    pub fn new(iov_ptr: UserInPtr<IoVec<Out>>, iov_count: usize) -> IoVecs<Out> {
        iov_ptr.read_iovecs(iov_count).unwrap()
    }
}
impl<P: Policy> UserInPtr<IoVec<P>> {
    pub fn read_iovecs(&self, count: usize) -> Result<IoVecs<P>> {
        if self.0.is_null() {
            return Err(Error::InvalidPointer);
        }
        // Linux caps the number of iovecs at `IOV_MAX` (1024); reject anything
        // larger so a huge `count` cannot trigger an unbounded allocation.
        if count > 1024 {
            return Err(Error::InvalidLength);
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

    /// Copy up to `buf.len()` bytes of the gathered iovec byte-stream into
    /// `buf`, starting at stream offset `offset`. Returns the number of bytes
    /// copied — 0 only at end-of-stream. Zero-length iovecs are legal and
    /// contribute nothing (POSIX).
    ///
    /// This is what lets `writev` process an arbitrarily large gather list
    /// through a bounded kernel buffer, the way Linux iterates the iov array
    /// with a bounded copy per step, instead of `read_to_vec`'s
    /// caller-controlled single allocation of the full total.
    pub fn read_bytes_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let mut skip = offset;
        let mut copied = 0usize;
        for vec in self.vec.iter() {
            if copied == buf.len() {
                break;
            }
            if skip >= vec.len {
                skip -= vec.len;
                continue;
            }
            let n = (vec.len - skip).min(buf.len() - copied);
            let src = vec.ptr.add(skip).as_slice(n)?;
            buf[copied..copied + n].copy_from_slice(src);
            copied += n;
            skip = 0;
        }
        Ok(copied)
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

    // Deliberate: this hands out a `&mut` view of a raw user-space pointer from
    // `&self`. Aliasing is the caller's responsibility (user memory, checked
    // separately), so the standard `mut_from_ref` guard does not apply.
    #[allow(clippy::mut_from_ref)]
    pub fn as_mut_slice(&self) -> Result<&mut [u8]> {
        if !self.ptr.is_null() {
            Ok(unsafe { core::slice::from_raw_parts_mut(self.ptr.0, self.len) })
        } else {
            Err(Error::InvalidVectorAddress)
        }
    }
}
