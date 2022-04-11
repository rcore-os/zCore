// 封装对外设地址空间的访问，包括内存映射 IO 和端口映射 IO。
//
// 要了解这两种访问外设的方式，查看[维基百科](https://en.wikipedia.org/wiki/Memory-mapped_I/O)。
//! Peripheral address space access, including memory-mapped IO and port-mapped IO.
//!
//! About these two methods of performing I/O, see [wikipedia](https://en.wikipedia.org/wiki/Memory-mapped_I/O).

use core::ops::{BitAnd, BitOr, Not};

mod mmio;
#[cfg(target_arch = "x86_64")]
mod pmio;

pub use mmio::Mmio;
#[cfg(target_arch = "x86_64")]
pub use pmio::Pmio;

// 用于处理外设地址空间访问的接口。
/// An interface for dealing with device address space access.
pub trait Io {
    // 可访问的对象的类型。
    /// The type of object to access.
    type Value: Copy
        + BitAnd<Output = Self::Value>
        + BitOr<Output = Self::Value>
        + Not<Output = Self::Value>;

    // 从外设读取值。
    /// Reads value from device.
    fn read(&self) -> Self::Value;

    // 向外设写入值。
    /// Writes `value` to device.
    fn write(&mut self, value: Self::Value);
}

// 外设地址空间的一个只读单元。
/// A readonly unit in device address space.
#[repr(transparent)]
pub struct ReadOnly<I>(I);

impl<I> ReadOnly<I> {
    // 构造外设地址空间的一个只读单元。
    /// Constructs a readonly unit in device address space.
    pub const fn new(inner: I) -> Self {
        Self(inner)
    }
}

impl<I: Io> ReadOnly<I> {
    // 从外设读取值。
    /// Reads value from device.
    #[inline(always)]
    pub fn read(&self) -> I::Value {
        self.0.read()
    }
}

// 外设地址空间的一个只写单元。
/// A write-only unit in device address space.
#[repr(transparent)]
pub struct WriteOnly<I>(I);

impl<I> WriteOnly<I> {
    // 构造外设地址空间的一个只写单元。
    /// Constructs a write-only unit in device address space.
    pub const fn new(inner: I) -> Self {
        Self(inner)
    }
}

impl<I: Io> WriteOnly<I> {
    // 向外设写入值。
    /// Writes `value` to device.
    #[inline(always)]
    pub fn write(&mut self, value: I::Value) {
        self.0.write(value);
    }
}
