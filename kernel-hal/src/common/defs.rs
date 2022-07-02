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
        const DEVICE    = 1 << 7;
        const RXW = Self::READ.bits | Self::WRITE.bits | Self::EXECUTE.bits;
    }
}
numeric_enum! {
    #[repr(u32)]
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
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

        #[derive(Debug, PartialEq, Eq, Copy, Clone)]
        pub enum Fault {
            AddressSize,
            Translation,
            AccessFlag,
            Permission,
            Alignment,
            TlbConflict,
            Other(u8),
        }

        impl From<u32> for Fault {
            fn from(val: u32) -> Fault {
                use self::Fault::*;

                // IFSC or DFSC bits (ref: D10.2.39, Page 2457~2464).
                match val & 0b111100 {
                    0b000000 => AddressSize,
                    0b000100 => Translation,
                    0b001000 => AccessFlag,
                    0b001100 => Permission,
                    0b100000 => Alignment,
                    0b110000 => TlbConflict,
                    _ => Other((val & 0b111111) as u8),
                }
            }
        }

        #[derive(Debug, PartialEq, Eq, Copy, Clone)]
        pub enum Syndrome {
            Unknown,
            WfiWfe,
            McrMrc,
            McrrMrrc,
            LdcStc,
            SimdFp,
            Vmrs,
            Mrrc,
            IllegalExecutionState,
            Svc(u16),
            Hvc(u16),
            Smc(u16),
            MsrMrsSystem,
            InstructionAbort { kind: Fault, level: u8 },
            PCAlignmentFault,
            DataAbort { kind: Fault, level: u8 },
            SpAlignmentFault,
            TrappedFpu,
            SError,
            Breakpoint,
            Step,
            Watchpoint,
            Brk(u16),
            Other(u32),
        }

        /// Converts a raw syndrome value (ESR) into a `Syndrome` (ref: D1.10.4, D10.2.39).
        impl From<u32> for Syndrome {
            fn from(esr: u32) -> Syndrome {
                use self::Syndrome::*;

                let ec = esr >> 26;
                let iss = esr & 0xFFFFFF;

                match ec {
                    0b000000 => Unknown,
                    0b000001 => WfiWfe,
                    0b000011 => McrMrc,
                    0b000100 => McrrMrrc,
                    0b000101 => McrMrc,
                    0b000110 => LdcStc,
                    0b000111 => SimdFp,
                    0b001000 => Vmrs,
                    0b001100 => Mrrc,
                    0b001110 => IllegalExecutionState,
                    0b010001 => Svc((iss & 0xFFFF) as u16),
                    0b010010 => Hvc((iss & 0xFFFF) as u16),
                    0b010011 => Smc((iss & 0xFFFF) as u16),
                    0b010101 => Svc((iss & 0xFFFF) as u16),
                    0b010110 => Hvc((iss & 0xFFFF) as u16),
                    0b010111 => Smc((iss & 0xFFFF) as u16),
                    0b011000 => MsrMrsSystem,
                    0b100000 | 0b100001 => InstructionAbort {
                        kind: Fault::from(iss),
                        level: (iss & 0b11) as u8,
                    },
                    0b100010 => PCAlignmentFault,
                    0b100100 | 0b100101 => DataAbort {
                        kind: Fault::from(iss),
                        level: (iss & 0b11) as u8,
                    },
                    0b100110 => SpAlignmentFault,
                    0b101000 => TrappedFpu,
                    0b101100 => TrappedFpu,
                    0b101111 => SError,
                    0b110000 => Breakpoint,
                    0b110001 => Breakpoint,
                    0b110010 => Step,
                    0b110011 => Step,
                    0b110100 => Watchpoint,
                    0b110101 => Watchpoint,
                    0b111100 => Brk((iss & 0xFFFF) as u16),
                    other => Other(other),
                }
            }
        }

    }
}

/// The smallest size of a page (4K).
pub const PAGE_SIZE: usize = super::vm::PageSize::Size4K as usize;

pub use super::addr::{DevVAddr, PhysAddr, VirtAddr};
