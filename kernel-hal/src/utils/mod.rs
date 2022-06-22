#[cfg(not(feature = "libos"))]
use core::cell::UnsafeCell;

cfg_if! {
    if #[cfg(not(feature = "libos"))] {
        pub(crate) mod page_table;
    }
}

pub(crate) mod init_once;

#[cfg(not(feature = "libos"))]
pub struct PerCpuCell<T>(pub UnsafeCell<T>);

#[cfg(not(feature = "libos"))]
// #Safety: Only the corresponding cpu will access it.
unsafe impl<T> Sync for PerCpuCell<T> {}

#[cfg(not(feature = "libos"))]
impl<T> PerCpuCell<T> {
    pub const fn new(t: T) -> Self {
        Self(UnsafeCell::new(t))
    }

    pub fn get(&self) -> &T {
        unsafe { &*self.0.get() }
    }

    #[allow(clippy::mut_from_ref)]
    pub fn get_mut(&self) -> &mut T {
        unsafe { &mut *self.0.get() }
    }
}
