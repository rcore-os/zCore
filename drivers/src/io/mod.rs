use core::ops::{BitAnd, BitOr, Not};

mod mmio;
#[cfg(target_arch = "x86_64")]
mod pio;

pub use mmio::Mmio;
#[cfg(target_arch = "x86_64")]
pub use pio::Pio;

pub trait Io {
    type Value: Copy
        + BitAnd<Output = Self::Value>
        + BitOr<Output = Self::Value>
        + Not<Output = Self::Value>;

    fn read(&self) -> Self::Value;
    fn write(&mut self, value: Self::Value);
}

#[repr(transparent)]
pub struct ReadOnly<I> {
    inner: I,
}

impl<I> ReadOnly<I> {
    pub const fn new(inner: I) -> ReadOnly<I> {
        ReadOnly { inner }
    }
}

impl<I: Io> ReadOnly<I> {
    #[inline(always)]
    pub fn read(&self) -> I::Value {
        self.inner.read()
    }
}

#[repr(transparent)]
pub struct WriteOnly<I> {
    inner: I,
}

impl<I> WriteOnly<I> {
    pub const fn new(inner: I) -> WriteOnly<I> {
        WriteOnly { inner }
    }
}

impl<I: Io> WriteOnly<I> {
    #[inline(always)]
    pub fn write(&mut self, value: I::Value) {
        self.inner.write(value)
    }
}
