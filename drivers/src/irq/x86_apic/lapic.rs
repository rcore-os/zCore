use x2apic::lapic::{
    xapic_base, LocalApic as LocalApicInner, LocalApicBuilder, TimerDivide, TimerMode,
};

use super::{consts, Phys2VirtFn};

static mut LOCAL_APIC: Option<LocalApic> = None;
static mut BSP_ID: Option<u8> = None;

pub struct LocalApic {
    inner: LocalApicInner,
}

impl LocalApic {
    pub unsafe fn get<'a>() -> &'a mut LocalApic {
        LOCAL_APIC
            .as_mut()
            .expect("Local APIC is not initialized by BSP")
    }

    pub unsafe fn init_bsp(phys_to_virt: Phys2VirtFn) {
        let base_vaddr = phys_to_virt(xapic_base() as usize);
        let mut inner = LocalApicBuilder::new()
            .timer_vector(consts::X86_INT_APIC_TIMER)
            .error_vector(consts::X86_INT_APIC_ERROR)
            .spurious_vector(consts::X86_INT_APIC_SPURIOUS)
            .set_xapic_base(base_vaddr as u64)
            .build()
            .unwrap_or_else(|err| panic!("{}", err));
        inner.enable();

        assert!(inner.is_bsp());
        BSP_ID = Some((inner.id() >> 24) as u8);
        LOCAL_APIC = Some(LocalApic { inner });
    }

    pub unsafe fn init_ap() {
        Self::get().inner.enable();
    }

    pub fn bsp_id() -> u8 {
        unsafe { BSP_ID.unwrap() }
    }

    pub fn id(&mut self) -> u8 {
        unsafe { (self.inner.id() >> 24) as u8 }
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
