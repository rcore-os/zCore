//! Kernel counter.
use core::fmt::{Debug, Error, Formatter};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Kernel counter.
#[repr(transparent)]
#[derive(Debug)]
pub struct KCounter(AtomicUsize);

impl KCounter {
    /// Create a new KCounter.
    pub const fn new() -> Self {
        KCounter(AtomicUsize::new(0))
    }

    /// Add a value to the counter.
    pub fn add(&self, x: usize) {
        self.0.fetch_add(x, Ordering::Relaxed);
    }

    /// Get the value of counter.
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

/// Kernel counter descriptor.
#[repr(C)]
pub struct KCounterDescriptor {
    /// The counter.
    pub counter: &'static KCounter,
    /// The counter's name.
    pub name: &'static str,
}

/// Define a new KCounter.
#[macro_export]
macro_rules! kcounter {
    ($var:ident, $name:expr) => {
        #[used]
        #[cfg_attr(target_os = "none", link_section = ".kcounter.items")]
        static $var: $crate::util::kcounter::KCounter = {
            #[used]
            #[cfg_attr(target_os = "none", link_section = ".kcounter.descriptor")]
            static DESCRIPTOR: $crate::util::kcounter::KCounterDescriptor =
                $crate::util::kcounter::KCounterDescriptor {
                    counter: &$var,
                    name: $name,
                };
            $crate::util::kcounter::KCounter::new()
        };
    };
}

/// Kernel counter descriptor array.
pub struct KCounterDescriptorArray(pub &'static [KCounterDescriptor]);

impl KCounterDescriptorArray {
    /// Get kcounter descriptor array from symbols.
    #[allow(unsafe_code)]
    pub fn get() -> Self {
        extern "C" {
            fn kcounter_descriptor_begin();
            fn kcounter_descriptor_end();
        }
        let start = kcounter_descriptor_begin as usize as *const KCounterDescriptor;
        let end = kcounter_descriptor_end as usize as *const KCounterDescriptor;
        let descs = unsafe { core::slice::from_raw_parts(start, end.offset_from(start) as usize) };
        KCounterDescriptorArray(descs)
    }
}

impl Debug for KCounterDescriptorArray {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.write_str("KCounters ")?;
        f.debug_map()
            .entries(self.0.iter().map(|desc| (desc.name, desc.counter.get())))
            .finish()
    }
}
