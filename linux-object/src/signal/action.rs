use crate::signal::Signal;
use _core::convert::TryFrom;
use bitflags::*;

pub const SIG_ERR: usize = usize::max_value() - 1;
pub const SIG_DFL: usize = 0;
pub const SIG_IGN: usize = 1;

/// Linux struct sigset_t
///
/// yet there's a bug because of mismatching bits: <https://sourceware.org/bugzilla/show_bug.cgi?id=25657>
/// just support 64bits size sigset
#[derive(Default, Clone, Copy, Debug)]
#[repr(C)]
pub struct Sigset(u64);

impl Sigset {
    pub fn new(val: u64) -> Self {
        Sigset(val)
    }
    pub fn empty() -> Self {
        Sigset(0)
    }
    pub fn val(&self) -> u64 {
        self.0
    }
    pub fn contains(&self, sig: Signal) -> bool {
        (self.0 & sig.as_bit()) != 0
    }
    pub fn insert(&mut self, sig: Signal) {
        self.0 |= sig.as_bit()
    }
    pub fn insert_set(&mut self, sigset: &Sigset) {
        self.0 |= sigset.0;
    }
    pub fn remove(&mut self, sig: Signal) {
        self.0 ^= self.0 & sig.as_bit();
    }
    pub fn remove_set(&mut self, sigset: &Sigset) {
        self.0 ^= self.0 & sigset.0;
    }
    pub fn mask_with(&self, mask: &Sigset) -> Sigset {
        Sigset(self.0 & (!mask.0))
    }
    pub fn find_first_signal(&self) -> Option<Signal> {
        let tz = self.0.trailing_zeros() as u8;
        if tz >= 64 {
            None
        } else {
            Some(Signal::try_from(tz + 1).unwrap())
        }
    }
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
    pub fn is_not_empty(&self) -> bool {
        self.0 != 0
    }
}

/// Linux struct sigaction
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SignalAction {
    pub handler: usize, // this field may be an union
    pub flags: SignalActionFlags,
    pub restorer: usize,
    pub mask: Sigset,
}

#[repr(C)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct SiginfoFields {
    pad: [u8; Self::PAD_SIZE],
    // TODO: fill this union
}

impl SiginfoFields {
    const PAD_SIZE: usize = 128 - 2 * core::mem::size_of::<i32>() - core::mem::size_of::<usize>();
}

impl Default for SiginfoFields {
    fn default() -> Self {
        SiginfoFields {
            pad: [0; Self::PAD_SIZE],
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct SigInfo {
    pub signo: i32,
    pub errno: i32,
    pub code: SignalCode,
    pub field: SiginfoFields,
}

impl Default for SigInfo {
    fn default() -> Self {
        Self {
            signo: 0,
            errno: 0,
            code: SignalCode::USER,
            field: Default::default(),
        }
    }
}

/// A code identifying the cause of the signal.
#[repr(i32)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum SignalCode {
    ASYNCNL = -60,
    TKILL = -6,
    SIGIO = -5,
    ASYNCIO = -4,
    MESGQ = -3,
    TIMER = -2,
    QUEUE = -1,
    /// from user
    USER = 0,
    /// from kernel
    KERNEL = 128,
}

bitflags! {
    #[derive(Default)]
    pub struct SignalActionFlags : usize {
        const NOCLDSTOP = 1;
        const NOCLDWAIT = 2;
        const SIGINFO = 4;
        const ONSTACK = 0x08000000;
        const RESTART = 0x10000000;
        const NODEFER = 0x40000000;
        const RESETHAND = 0x80000000;
        const RESTORER = 0x04000000;
    }
}
