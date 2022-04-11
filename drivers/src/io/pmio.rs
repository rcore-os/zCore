// 端口映射 I/O。
//! Port-mapped I/O.

use super::Io;
use core::{arch::asm, marker::PhantomData};

// 端口映射 I/O。
/// Port-mapped I/O.
#[derive(Copy, Clone)]
pub struct Pmio<T> {
    port: u16,
    _phantom: PhantomData<T>,
}

impl<T> Pmio<T> {
    // 映射指定端口进行外设访问。
    /// Maps a given port to assess device.
    pub const fn new(port: u16) -> Self {
        Self {
            port,
            _phantom: PhantomData,
        }
    }
}

// 逐字节端口映射读写。
/// Read/Write for byte PMIO.
impl Io for Pmio<u8> {
    type Value = u8;

    // 读。
    /// Read.
    #[inline(always)]
    fn read(&self) -> u8 {
        let value: u8;
        unsafe {
            asm!("in al, dx", out("al") value, in("dx") self.port, options(nomem, nostack, preserves_flags));
        }
        value
    }

    // 写。
    /// Write.
    #[inline(always)]
    fn write(&mut self, value: u8) {
        unsafe {
            asm!("out dx, al", in("al") value, in("dx") self.port, options(nomem, nostack, preserves_flags));
        }
    }
}

// 逐字端口映射读写。
/// Read/Write for word PMIO.
impl Io for Pmio<u16> {
    type Value = u16;

    // 读。
    /// Read.
    #[inline(always)]
    fn read(&self) -> u16 {
        let value: u16;
        unsafe {
            asm!("in ax, dx", out("ax") value, in("dx") self.port, options(nomem, nostack, preserves_flags));
        }
        value
    }

    // 写。
    /// Write.
    #[inline(always)]
    fn write(&mut self, value: u16) {
        unsafe {
            asm!("out dx, ax", in("ax") value, in("dx") self.port, options(nomem, nostack, preserves_flags));
        }
    }
}

// 逐双字端口映射读写。
/// Read/Write for double-word PMIO.
impl Io for Pmio<u32> {
    type Value = u32;

    // 读。
    /// Read.
    #[inline(always)]
    fn read(&self) -> u32 {
        let value: u32;
        unsafe {
            asm!("in eax, dx", out("eax") value, in("dx") self.port, options(nomem, nostack, preserves_flags));
        }
        value
    }

    // 写。
    /// Write.
    #[inline(always)]
    fn write(&mut self, value: u32) {
        unsafe {
            asm!("out dx, eax", in("eax") value, in("dx") self.port, options(nomem, nostack, preserves_flags));
        }
    }
}
