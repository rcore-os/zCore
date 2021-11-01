//! Kernel counter.

use core::fmt::{Debug, Error, Formatter};
use core::slice::from_raw_parts;
use core::sync::atomic::{AtomicUsize, Ordering};

const KCOUNTER_MAGIC: u64 = 1_547_273_975;

/// Kernel counter type.
#[repr(u64)]
#[derive(Debug)]
#[allow(dead_code)]
enum DescriptorType {
    /// Padding
    Padding = 0,
    /// Sum
    Sum = 1,
    /// Min
    Min = 2,
    /// Max
    Max = 3,
}

/// Kernel counter descriptor.
#[repr(C)]
#[derive(Debug)]
pub struct Descriptor {
    name: [u8; 56],
    desc_type: DescriptorType,
}

impl Descriptor {
    /// Create a kcounter descriptor by `name`.
    pub const fn new(name: &'static str) -> Self {
        macro_rules! try_fill_char {
            (@1, $dst: ident, $src: ident, $idx: expr) => {
                if $src.len() > $idx {
                    $dst[$idx] = $src[$idx];
                }
            };
            (@4, $dst: ident, $src: ident, $idx: expr) => {
                try_fill_char!(@1, $dst, $src, $idx);
                try_fill_char!(@1, $dst, $src, $idx + 1);
                try_fill_char!(@1, $dst, $src, $idx + 2);
                try_fill_char!(@1, $dst, $src, $idx + 3);
            };
            (@16, $dst: ident, $src: ident, $idx: expr) => {
                try_fill_char!(@4, $dst, $src, $idx);
                try_fill_char!(@4, $dst, $src, $idx + 4);
                try_fill_char!(@4, $dst, $src, $idx + 8);
                try_fill_char!(@4, $dst, $src, $idx + 12);
            };
        }
        macro_rules! str_to_array56 {
            ($str: expr) => {{
                let bytes = $str.as_bytes();
                let mut arr = [0; 56];
                try_fill_char!(@16, arr, bytes, 0);
                try_fill_char!(@16, arr, bytes, 16);
                try_fill_char!(@16, arr, bytes, 32);
                try_fill_char!(@4, arr, bytes, 48);
                try_fill_char!(@4, arr, bytes, 52);
                arr
            }};
        }
        Self {
            name: str_to_array56!(name),
            desc_type: DescriptorType::Sum,
        }
    }
}

/// Kernel counter.
#[derive(Debug)]
#[repr(transparent)]
pub struct Counter(AtomicUsize);

impl Counter {
    /// Create a new KCounter.
    pub const fn new() -> Self {
        Counter(AtomicUsize::new(0))
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

/// Head of the descriptor table.
#[repr(C)]
#[derive(Debug)]
pub struct DescriptorVmoHeader {
    magic: u64,
    max_cpus: u64,
    descriptor_table_size: usize,
}

impl Default for DescriptorVmoHeader {
    fn default() -> Self {
        Self {
            magic: KCOUNTER_MAGIC,
            max_cpus: 1,
            descriptor_table_size: 0,
        }
    }
}

/// Kernel counters array.
pub struct AllCounters {
    desc: &'static [Descriptor],
    counters: &'static [Counter],
}

#[allow(unsafe_code)]
impl AllCounters {
    /// Get kcounter descriptor table from symbols.
    pub fn get() -> Self {
        let desc_start = kcounters_desc_start as usize as *const Descriptor;
        let desc_end = kcounters_desc_end as usize as *const Descriptor;
        let desc = unsafe { from_raw_parts(desc_start, desc_end.offset_from(desc_start) as _) };

        let arena_start = kcounters_arena_start as usize as *const Counter;
        let arena_end = kcounters_arena_end as usize as *const Counter;
        let counters =
            unsafe { from_raw_parts(arena_start, arena_end.offset_from(arena_start) as _) };

        Self { desc, counters }
    }

    /// Data of the kcounter descriptor VMO, consists of the [`DescriptorVmoHeader`]
    /// and an table of [`Descriptor`].
    pub fn raw_desc_vmo_data() -> &'static [u8] {
        let desc_vmo_start = kcounters_desc_vmo_start as usize;
        let desc_vmo_end = kcounters_desc_end as usize;
        unsafe { from_raw_parts(desc_vmo_start as *const _, desc_vmo_end - desc_vmo_start) }
    }

    /// Data of the kcounter arena VMO, consists of the [`DescriptorVmoHeader`]
    /// and an table of [`Descriptor`].
    pub fn raw_arena_vmo_data() -> &'static [u8] {
        let arena_vmo_start = kcounters_arena_start as usize;
        let arena_vmo_end = kcounters_arena_end as usize;
        unsafe { from_raw_parts(arena_vmo_start as *const _, arena_vmo_end - arena_vmo_start) }
    }
}

impl Debug for AllCounters {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.write_str("AllCounters ")?;
        f.debug_map()
            .entries(self.desc.iter().zip(self.counters).map(|(desc, counter)| {
                let name = &desc.name;
                let len = name.iter().position(|&c| c == b'\0').unwrap_or(name.len());
                (core::str::from_utf8(&name[..len]).unwrap(), counter.get())
            }))
            .finish()
    }
}

extern "C" {
    fn kcounters_desc_vmo_start();
    fn kcounters_desc_start();
    fn kcounters_desc_end();
    fn kcounters_arena_start();
    fn kcounters_arena_end();
}

#[used]
#[cfg_attr(target_os = "none", link_section = ".kcounter.desc.header")]
static DESCRIPTOR_VMO_HEADER: [u64; 2] = [
    KCOUNTER_MAGIC, // magic
    1,              // max_cpus
                    // descriptor_table_size is filled in linker.ld
];

/// Define a new kernel counter.
#[macro_export]
macro_rules! kcounter {
    ($var:ident, $name:expr) => {
        #[used]
        #[cfg_attr(target_os = "none", link_section = concat!(".bss.kcounter.", $name))]
        static $var: $crate::util::kcounter::Counter = {
            use $crate::util::kcounter::{Counter, Descriptor};
            #[used]
            #[cfg_attr(target_os = "none", link_section = concat!(".kcounter.desc.", $name))]
            static DESCRIPTOR: Descriptor = Descriptor::new($name);
            Counter::new()
        };
    };
}
