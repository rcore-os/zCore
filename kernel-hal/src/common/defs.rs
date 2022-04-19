use bitflags::bitflags;
use numeric_enum_macro::numeric_enum;

/// The error type which is returned from HAL functions.
/// TODO: more error types.
#[derive(Debug)]
pub struct HalError;

/// The result type returned by HAL functions.
pub type HalResult<T = ()> = core::result::Result<T, HalError>;

bitflags! {
    /// Generic memory flags.
    pub struct MMUFlags: usize {
        #[allow(clippy::identity_op)]
        const CACHE_1   = 1 << 0;
        const CACHE_2   = 1 << 1;
        const READ      = 1 << 2;
        const WRITE     = 1 << 3;
        const EXECUTE   = 1 << 4;
        const USER      = 1 << 5;
        const HUGE_PAGE = 1 << 6;
        const RXW = Self::READ.bits | Self::WRITE.bits | Self::EXECUTE.bits;
    }
}
numeric_enum! {
    #[repr(u32)]
    #[derive(Debug, PartialEq, Clone, Copy)]
    /// Generic cache policy.
    pub enum CachePolicy {
        Cached = 0,
        Uncached = 1,
        UncachedDevice = 2,
        WriteCombining = 3,
    }
}

cfg_if! {
    if #[cfg(target_arch = "aarch64")] {
        #[derive(Debug, Eq, PartialEq)]
        pub enum IrqHandlerResult {
            Reschedule,
            NoReschedule,
        }

        #[derive(Debug, PartialEq, Eq, Copy, Clone)]
        pub enum Kind {
            Synchronous = 0,
            Irq = 1,
            Fiq = 2,
            SError = 3,
        }

        impl Kind {
            pub fn from(x: usize) -> Kind {
                match x {
                    x if x == Kind::Synchronous as usize => Kind::Synchronous,
                    x if x == Kind::Irq as usize => Kind::Irq,
                    x if x == Kind::Fiq as usize => Kind::Fiq,
                    x if x == Kind::SError as usize => Kind::SError,
                    _ => panic!("bad kind"),
                }
            }
        }

        #[derive(Debug, PartialEq, Eq, Copy, Clone)]
        pub enum Source {
            CurrentSpEl0 = 0,
            CurrentSpElx = 1,
            LowerAArch64 = 2,
            LowerAArch32 = 3,
        }

        impl Source {
            pub fn from(x: usize) -> Source {
                match x {
                    x if x == Source::CurrentSpEl0 as usize => Source::CurrentSpEl0,
                    x if x == Source::CurrentSpElx as usize => Source::CurrentSpElx,
                    x if x == Source::LowerAArch64 as usize => Source::LowerAArch64,
                    x if x == Source::LowerAArch32 as usize => Source::LowerAArch32,
                    _ => panic!("bad kind"),
                }
            }
        }

        #[derive(Debug, PartialEq, Eq, Copy, Clone)]
        pub struct Info {
            pub source: Source,
            pub kind: Kind,
        }
    }
}

/// The smallest size of a page (4K).
pub const PAGE_SIZE: usize = super::vm::PageSize::Size4K as usize;

pub use super::addr::{DevVAddr, PhysAddr, VirtAddr};
