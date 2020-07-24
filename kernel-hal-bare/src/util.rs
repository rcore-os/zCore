use core::ptr::{read_volatile, write_volatile};

#[inline(always)]
pub fn write<T>(addr: usize, content: T) {
    let cell = (addr) as *mut T;
    unsafe {
        write_volatile(cell, content);
    }
}

#[inline(always)]
pub fn read<T>(addr: usize) -> T {
    let cell = (addr) as *const T;
    unsafe { read_volatile(cell) }
}
