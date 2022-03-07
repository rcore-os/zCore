use core::cell::{RefCell, RefMut};
use lazy_static::*;

mod lock_ffi {
    extern "C" {
        pub fn intr_on();
        pub fn intr_off();
        pub fn intr_get() -> bool;
        pub fn cpu_id() -> u8;
    }
}

pub fn intr_on() {
    unsafe {
        lock_ffi::intr_on();
    }
}

pub fn intr_off() {
    unsafe {
        lock_ffi::intr_off();
    }
}

pub fn intr_get() -> bool {
    unsafe { lock_ffi::intr_get() }
}

pub fn cpu_id() -> u8 {
    unsafe { lock_ffi::cpu_id() }
}

#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct Cpu {
    pub noff: i32,              // Depth of push_off() nesting.
    pub interrupt_enable: bool, // Were interrupts enabled before push_off()?
}

impl Cpu {
    const fn new() -> Self {
        Self {
            noff: 0,
            interrupt_enable: false,
        }
    }
}

pub struct SafeRefCell<T>(RefCell<T>);

// #Safety: Only the corresponding cpu will access it.
unsafe impl<Cpu> Sync for SafeRefCell<Cpu> {}

impl<T> SafeRefCell<T> {
    const fn new(t: T) -> Self {
        Self(RefCell::new(t))
    }
}

const DEFAULT_CPU: SafeRefCell<Cpu> = SafeRefCell::new(Cpu::new());

lazy_static! {
    pub static ref CPUS: [SafeRefCell<Cpu>; 16] = [DEFAULT_CPU; 16];
}

pub fn mycpu() -> RefMut<'static, Cpu> {
    return CPUS[cpu_id() as usize].0.borrow_mut();
}

// push_off/pop_off are like intr_off()/intr_on() except that they are matched:
// it takes two pop_off()s to undo two push_off()s.  Also, if interrupts
// are initially off, then push_off, pop_off leaves them off.
pub(crate) fn push_off() {
    let old = intr_get();
    intr_off();
    let mut cpu = mycpu();
    if cpu.noff == 0 {
        cpu.interrupt_enable = old;
    }
    cpu.noff += 1;
}

pub(crate) fn pop_off() {
    let mut cpu = mycpu();
    if intr_get() || cpu.noff < 1 {
        panic!("pop_off");
    }
    cpu.noff -= 1;
    if cpu.noff == 0 && cpu.interrupt_enable {
        intr_on();
    }
}
