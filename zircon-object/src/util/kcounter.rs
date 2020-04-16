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

pub const K_SUM: u64 = 1;

#[repr(C)]
pub struct KCounterDesc {
    pub name: [u8; 56],
    pub type_: u64,
}

impl KCounterDescriptor {
    pub fn gen_desc(&self) -> KCounterDesc{
        let mut name = [0u8; 56];
        let length = self.name.len().min(56);
        name[..length].copy_from_slice(&self.name.as_bytes()[..length]);
        KCounterDesc {
            name,
            type_: K_SUM,
        }
    }
}

#[repr(C)]
pub struct KCounterDescriptor {
    pub counter: &'static KCounter,
    pub name: &'static str,
}

#[macro_export]
macro_rules! kcounter {
    ($var:ident, $name:expr) => {
        #[used]
        #[cfg_attr(target_os = "none", link_section = ".bss.kcounter.items")]
        static $var: $crate::util::kcounter::KCounter = {
            #[used]
            #[cfg_attr(target_os = "none", link_section = ".kcounter.descriptor.header")]
            static DESCRIPTOR: $crate::util::kcounter::KCounterDescriptor =
                $crate::util::kcounter::KCounterDescriptor {
                    counter: &$var,
                    name: $name,
                };
            $crate::util::kcounter::KCounter::new()
        };
    };
}
