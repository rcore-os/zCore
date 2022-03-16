use crate::signal::Signal;
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
    /// Creates a Sigset that yields nothing.
    pub fn empty() -> Self {
        Sigset(0)
    }
    /// Returns true if the given pattern matches a Signal of the Sigset.
    ///
    /// Returns false if it does not.
    pub fn contains(&self, sig: Signal) -> bool {
        (self.0 >> sig as u64 & 1) != 0
    }
    /// Inserts a Signal into the Sigset.
    pub fn insert(&mut self, sig: Signal) {
        self.0 |= 1 << sig as u64;
    }
    /// Inserts a sub-Sigset into the Sigset.
    pub fn insert_set(&mut self, sigset: &Sigset) {
        self.0 |= sigset.0;
    }
    /// Remove a Signal from the Sigset.
    pub fn remove(&mut self, sig: Signal) {
        self.0 ^= self.0 & (1 << sig as u64);
    }
    /// Remove a sub-Sigset from the Sigset.
    pub fn remove_set(&mut self, sigset: &Sigset) {
        self.0 ^= self.0 & sigset.0;
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
#[derive(Copy, Clone)]
pub union SiginfoFields {
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

/// signal infomation
#[repr(C)]
#[derive(Copy, Clone)]
pub struct SigInfo {
    pub signo: i32,
    pub errno: i32,
    pub code: SignalCode,
    pub field: SiginfoFields,
}

/// A code identifying the cause of the signal.
#[repr(i32)]
#[derive(Debug, Copy, Clone)]
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
