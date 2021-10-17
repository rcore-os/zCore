#![allow(dead_code)]

use spin::Once;

pub struct InitOnce<T> {
    inner: Once<T>,
    default: Option<T>,
}

impl<T> InitOnce<T> {
    pub const fn new() -> Self {
        Self {
            inner: Once::new(),
            default: None,
        }
    }

    pub const fn new_with_default(value: T) -> Self {
        Self {
            inner: Once::new(),
            default: Some(value),
        }
    }

    pub fn init_once_by(&self, value: T) {
        self.inner.call_once(|| value);
    }

    pub fn init_once<F>(&self, f: F)
    where
        F: FnOnce() -> T,
    {
        self.inner.call_once(f);
    }

    pub fn default(&self) -> Option<&T> {
        self.default.as_ref()
    }

    pub fn try_get(&self) -> Option<&T> {
        self.inner.get()
    }

    pub fn is_completed(&self) -> bool {
        self.inner.is_completed()
    }
}

impl<T> core::ops::Deref for InitOnce<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.inner
            .get()
            .or_else(|| self.default())
            .unwrap_or_else(|| panic!("uninitialized InitOnce<{}>", core::any::type_name::<T>()))
    }
}
