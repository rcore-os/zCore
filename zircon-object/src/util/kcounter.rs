use core::sync::atomic::{AtomicUsize, Ordering};

#[repr(transparent)]
#[derive(Debug)]
pub struct KCounter(AtomicUsize);

impl KCounter {
    pub const fn new() -> Self {
        KCounter(AtomicUsize::new(0))
    }

    pub fn add(&self, x: usize) {
        self.0.fetch_add(x, Ordering::Relaxed);
    }

    pub fn get(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct KCounterDesc {
    pub name: &'static str,
    pub kcounter: &'static KCounter,
}

#[macro_export]
macro_rules! kcounter {
    ($var:ident, $name:expr) => {
        #[cfg_attr(target_os = "none", link_section = ".bss.kcounter")]
        static $var: $crate::util::kcounter::KCounter = {
            #[used]
            #[cfg_attr(target_os = "none", link_section = ".kcountdesc.desc")]
            static DESC: $crate::util::kcounter::KCounterDesc =
                $crate::util::kcounter::KCounterDesc {
                    name: $name,
                    kcounter: &$var,
                };
            $crate::util::kcounter::KCounter::new()
        };
    };
}
