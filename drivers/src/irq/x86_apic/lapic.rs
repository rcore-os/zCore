use x2apic::lapic::{
    xapic_base, LocalApic as LocalApicInner, LocalApicBuilder, TimerDivide, TimerMode,
};

use super::{consts, Phys2VirtFn};
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

// APIC MMIO addresses are CPU-local, but the driver's mutable configuration
// must also be private to each CPU.
static mut LOCAL_APICS: [Option<LocalApic>; 256] = [const { None }; 256];
static APIC_BASE: AtomicUsize = AtomicUsize::new(0);
static BSP_ID: AtomicU8 = AtomicU8::new(0);

fn cpu_id() -> u8 {
    raw_cpuid::CpuId::new()
        .get_feature_info()
        .unwrap()
        .initial_local_apic_id()
}

pub struct LocalApic {
    inner: LocalApicInner,
}

impl LocalApic {
    pub unsafe fn get<'a>() -> &'a mut LocalApic {
        unsafe {
            let local_apic = (&raw mut LOCAL_APICS)
                .cast::<Option<LocalApic>>()
                .add(cpu_id() as usize);
            (*local_apic)
                .as_mut()
                .expect("Local APIC is not initialized by BSP")
        }
    }

    pub unsafe fn init_bsp(phys_to_virt: Phys2VirtFn) {
        unsafe {
            let base_vaddr = phys_to_virt(xapic_base() as usize);
            APIC_BASE.store(base_vaddr, Ordering::Release);
            Self::init_current(base_vaddr);
            assert!(Self::get().inner.is_bsp());
            BSP_ID.store(cpu_id(), Ordering::Release);
        }
    }

    unsafe fn init_current(base_vaddr: usize) {
        unsafe {
            let mut inner = LocalApicBuilder::new()
                .timer_vector(consts::X86_INT_APIC_TIMER)
                .error_vector(consts::X86_INT_APIC_ERROR)
                .spurious_vector(consts::X86_INT_APIC_SPURIOUS)
                .set_xapic_base(base_vaddr as u64)
                .build()
                .unwrap_or_else(|err| panic!("{}", err));
            inner.enable();

            let slot = (&raw mut LOCAL_APICS)
                .cast::<Option<LocalApic>>()
                .add(cpu_id() as usize);
            slot.write(Some(LocalApic { inner }));
        }
    }

    pub unsafe fn init_ap() {
        unsafe {
            Self::init_current(APIC_BASE.load(Ordering::Acquire));
        }
    }

    pub fn bsp_id() -> u8 {
        BSP_ID.load(Ordering::Acquire)
    }

    pub fn id(&mut self) -> u8 {
        cpu_id()
    }

    pub fn eoi(&mut self) {
        unsafe { self.inner.end_of_interrupt() }
    }

    pub fn disable_timer(&mut self) {
        unsafe { self.inner.disable_timer() }
    }

    pub fn enable_timer(&mut self) {
        unsafe { self.inner.enable_timer() }
    }

    pub fn set_timer_mode(&mut self, mode: TimerMode) {
        unsafe { self.inner.set_timer_mode(mode) }
    }

    pub fn set_timer_divide(&mut self, divide: TimerDivide) {
        unsafe { self.inner.set_timer_divide(divide) }
    }

    pub fn set_timer_initial(&mut self, initial: u32) {
        unsafe { self.inner.set_timer_initial(initial) }
    }
}
