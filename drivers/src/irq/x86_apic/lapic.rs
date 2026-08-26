use x2apic::lapic::{
    xapic_base, IpiAllShorthand, LocalApic as LocalApicInner, LocalApicBuilder, TimerDivide,
    TimerMode,
};

use super::{consts, Phys2VirtFn};

static mut LOCAL_APIC: Option<LocalApic> = None;
static mut BSP_ID: Option<u32> = None;

/// `IA32_APIC_BASE` bit 10: the Local APIC is in x2APIC mode.
///
/// [`LocalApicBuilder::build`] selects x2APIC whenever the CPU *supports* it
/// (`CPUID.01H:ECX[21]`) — which is every x86 since roughly 2008 — and
/// `enable()` then sets this bit. Once it is set the LAPIC no longer decodes
/// its MMIO page and every register moves to the MSR interface, which changes
/// both the ICR destination encoding and the layout of the ID register.
/// QEMU's default TCG CPU does not advertise x2APIC, so emulated boots stay on
/// the xAPIC path and never exercise the difference.
fn x2apic_active() -> bool {
    const IA32_APIC_BASE: u32 = 0x1B;
    const EXTD: u64 = 1 << 10;
    let apic_base = unsafe { x86_64::registers::model_specific::Msr::new(IA32_APIC_BASE).read() };
    apic_base & EXTD != 0
}

pub struct LocalApic {
    inner: LocalApicInner,
}

impl LocalApic {
    pub fn is_initialized() -> bool {
        unsafe { (*core::ptr::addr_of!(LOCAL_APIC)).is_some() }
    }

    pub unsafe fn get<'a>() -> &'a mut LocalApic {
        (*core::ptr::addr_of_mut!(LOCAL_APIC))
            .as_mut()
            .expect("Local APIC is not initialized by BSP")
    }

    pub unsafe fn init_bsp(phys_to_virt: Phys2VirtFn) {
        let base_vaddr = phys_to_virt(xapic_base() as usize);
        let mut inner = match LocalApicBuilder::new()
            .timer_vector(consts::X86_INT_APIC_TIMER)
            .error_vector(consts::X86_INT_APIC_ERROR)
            .spurious_vector(consts::X86_INT_APIC_SPURIOUS)
            .set_xapic_base(base_vaddr as u64)
            .build()
        {
            Ok(lapic) => lapic,
            Err(e) => {
                // A LAPIC build failure is a critical issue but not necessarily
                // a hard stop on all hardware — log it and attempt to continue
                // rather than panicking and leaving the screen frozen at 80%.
                crate::klog_err!(
                    "[lapic] LocalApicBuilder::build() failed: {} — continuing without LAPIC",
                    e
                );
                return;
            }
        };
        inner.enable();

        if !inner.is_bsp() {
            crate::klog_warn!(
                "[lapic] init_bsp() on non-BSP core (id={:#x}); APIC routing may be incorrect",
                Self::decode_id(inner.id())
            );
        }
        let bsp_id = Self::decode_id(inner.id());
        crate::klog_info!(
            "[lapic] BSP APIC id {:#x}, mode {}",
            bsp_id,
            if x2apic_active() { "x2APIC" } else { "xAPIC" }
        );
        BSP_ID = Some(bsp_id);
        LOCAL_APIC = Some(LocalApic { inner });
    }

    pub unsafe fn init_ap() {
        Self::get().inner.enable();
    }

    /// Normalise the raw ID-register value into a hardware APIC ID.
    ///
    /// xAPIC keeps the id in bits 31:24 of the MMIO register at offset 0x20;
    /// x2APIC's `IA32_X2APIC_APICID` (MSR 0x802) holds the full 32-bit id with
    /// no shift. Shifting unconditionally reported id 0 for every CPU on an
    /// x2APIC machine.
    fn decode_id(raw: u32) -> u32 {
        if x2apic_active() {
            raw
        } else {
            raw >> 24
        }
    }

    /// APIC ID of the boot processor, truncated to the 8 bits that a legacy
    /// (non-remapped) IOAPIC redirection entry can carry in physical
    /// destination mode.
    pub fn bsp_id() -> u8 {
        unsafe { BSP_ID.unwrap_or(0) as u8 }
    }

    pub fn id(&mut self) -> u32 {
        unsafe { Self::decode_id(self.inner.id()) }
    }

    /// Encode an APIC id for the ICR destination as the x2apic crate expects.
    ///
    /// The crate writes `dest` into ICR bits 63:32 verbatim, which is the
    /// x2APIC layout. In xAPIC (MMIO) mode the destination lives in
    /// ICR_HIGH[31:24], so the id must be shifted — otherwise every IPI is
    /// delivered to APIC id 0 (the BSP), and an INIT/SIPI sequence aimed at
    /// an AP resets the boot processor instead (endless reboot loop on
    /// machines/emulators without x2APIC).
    fn icr_dest(dest: u32) -> u32 {
        if x2apic_active() {
            dest
        } else {
            // xAPIC physical destination is ICR_HIGH[31:24], so it can only
            // address ids 0..=255.
            (dest & 0xFF) << 24
        }
    }

    pub fn send_init_ipi(&mut self, dest: u32) {
        unsafe { self.inner.send_init_ipi(Self::icr_dest(dest)) }
    }

    pub fn send_sipi(&mut self, vector: u8, dest: u32) {
        unsafe { self.inner.send_sipi(vector, Self::icr_dest(dest)) }
    }

    pub fn send_ipi_to(&mut self, vector: u8, dest: u32) {
        unsafe { self.inner.send_ipi(vector, Self::icr_dest(dest)) }
    }

    /// [diag] Send an NMI to every other CPU. NMIs are delivered even to a core
    /// running with interrupts disabled (e.g. spinning on a lock), so this is the
    /// way to interrupt a wedged core and capture where it is stuck.
    pub fn send_nmi_all_others(&mut self) {
        unsafe { self.inner.send_nmi_all(IpiAllShorthand::AllExcludingSelf) }
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
