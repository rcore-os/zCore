mod consts;
mod ioapic;
mod lapic;

use self::consts::{X86_INT_BASE, X86_INT_LOCAL_APIC_BASE};
use self::ioapic::{IoApic, IoApicList};
use self::lapic::LocalApic;
use crate::prelude::{IrqHandler, IrqPolarity, IrqTriggerMode};
use crate::scheme::{IrqScheme, Scheme};
use crate::{utils::IrqManager, DeviceError, DeviceResult, PhysAddr, VirtAddr};
use core::ops::Range;
use lock::Mutex;

const IOAPIC_IRQ_RANGE: Range<usize> = X86_INT_BASE..X86_INT_LOCAL_APIC_BASE;
const LAPIC_IRQ_RANGE: Range<usize> = 0..16;

type Phys2VirtFn = fn(paddr: PhysAddr) -> VirtAddr;

/// Advanced Programmable Interrupt Controller
pub struct Apic {
    ioapic_list: IoApicList,
    manager_ioapic: Mutex<IrqManager<256>>,
    manager_lapic: Mutex<IrqManager<16>>,
}

impl Apic {
    /// Construct a new `Apic`.
    pub fn new(acpi_rsdp: usize, phys_to_virt: Phys2VirtFn) -> Self {
        Self {
            ioapic_list: IoApicList::new(acpi_rsdp, phys_to_virt),
            manager_ioapic: Mutex::new(IrqManager::new(IOAPIC_IRQ_RANGE)),
            manager_lapic: Mutex::new(IrqManager::new(LAPIC_IRQ_RANGE)),
        }
    }

    fn with_ioapic<F>(&self, gsi: u32, op: F) -> DeviceResult
    where
        F: FnOnce(&IoApic) -> DeviceResult,
    {
        if let Some(apic) = self.ioapic_list.find(gsi) {
            op(apic)
        } else {
            error!(
                "cannot find IOAPIC for global system interrupt number {}",
                gsi
            );
            Err(DeviceError::InvalidParam)
        }
    }

    pub fn send_init_ipi(dest: u32) {
        if LocalApic::is_initialized() {
            Self::local_apic().send_init_ipi(dest);
        }
    }

    pub fn send_sipi(vector: u8, dest: u32) {
        if LocalApic::is_initialized() {
            Self::local_apic().send_sipi(vector, dest);
        }
    }

    pub fn send_ipi_to(vector: u8, dest: u32) {
        if LocalApic::is_initialized() {
            Self::local_apic().send_ipi_to(vector, dest);
        }
    }

    /// [diag] Broadcast an NMI to every other CPU (reaches cores spinning with
    /// interrupts disabled).
    pub fn send_nmi_all_others() {
        if LocalApic::is_initialized() {
            Self::local_apic().send_nmi_all_others();
        }
    }

    pub fn init_local_apic_bsp(phys_to_virt: Phys2VirtFn) {
        unsafe { LocalApic::init_bsp(phys_to_virt) }
    }

    pub fn init_local_apic_ap() {
        if LocalApic::is_initialized() {
            unsafe { LocalApic::init_ap() }
        }
    }

    pub fn local_apic_ready() -> bool {
        LocalApic::is_initialized()
    }

    pub fn local_apic<'a>() -> &'a mut LocalApic {
        unsafe { LocalApic::get() }
    }

    pub fn register_local_apic_handler(&self, vector: usize, handler: IrqHandler) -> DeviceResult {
        if vector >= X86_INT_LOCAL_APIC_BASE {
            self.manager_lapic
                .lock()
                .register_handler(vector - X86_INT_LOCAL_APIC_BASE, handler)?;
            Ok(())
        } else {
            error!("invalid local APIC interrupt vector {}", vector);
            Err(DeviceError::InvalidParam)
        }
    }
}

impl Scheme for Apic {
    fn name(&self) -> &str {
        "x86-apic"
    }

    fn handle_irq(&self, vector: usize) {
        // Intel: the spurious-interrupt vector must NOT write EOI.
        if vector != self::consts::X86_INT_APIC_SPURIOUS && LocalApic::is_initialized() {
            Self::local_apic().eoi();
        }
        // CRITICAL: look the handler up under the manager lock, then RELEASE the
        // lock before invoking it. `manager_lapic`/`manager_ioapic` are single
        // global Mutexes taken on every interrupt (the LAPIC timer fires on
        // every CPU at 250 Hz), and the handlers re-enter this very path:
        // `timer_tick` runs a timer callback that can touch the IRQ subsystem,
        // and the old code (call under the lock) then re-acquired this same
        // global lock on the SAME CPU — a self-deadlock that pinned the CPU (and
        // the timer heap lock) forever and froze every other core. This never
        // reproduced under 2 emulated CPUs; it only bites on real multi-core
        // hardware. Cloning the `Arc` out keeps the closure alive even if
        // another CPU unregisters it while it runs.
        if vector == self::consts::X86_INT_APIC_SPURIOUS {
            return;
        }
        let handler = if vector >= X86_INT_LOCAL_APIC_BASE {
            self.manager_lapic
                .lock()
                .get(vector - X86_INT_LOCAL_APIC_BASE)
        } else {
            self.manager_ioapic.lock().get(vector)
        };
        match handler {
            Some(f) => {
                // Sticky smash / null stack-top: same policy as timer_tick —
                // device IRQs (PS/2/UART/xHCI) were still calling through after
                // a null-[rsp] soft-smash with in_timer_callback=false.
                if crate::utils::heap_smash_suspected() {
                    core::mem::forget(f);
                    return;
                }
                // Same check as EventListener / timer: null or non-kernel
                // vtable → skip + leak.
                if crate::utils::dyn_fat_ptr_live(&f) {
                    f();
                } else {
                    core::mem::forget(f);
                    warn!(
                        "IRQ vector {}: handler fat-pointer is dead (heap smash?); \
                         skipping to avoid null-range EXECUTE #PF",
                        vector
                    );
                }
            }
            None => warn!("no registered handler for interrupt vector {}!", vector),
        }
    }
}

impl IrqScheme for Apic {
    fn is_valid_irq(&self, gsi: usize) -> bool {
        self.ioapic_list.find(gsi as _).is_some()
            || (X86_INT_BASE..X86_INT_LOCAL_APIC_BASE).contains(&gsi)
    }

    fn mask(&self, gsi: usize) -> DeviceResult {
        if let Some(apic) = self.ioapic_list.find(gsi as _) {
            apic.toggle(gsi as _, false);
            Ok(())
        } else if (X86_INT_BASE..X86_INT_LOCAL_APIC_BASE).contains(&gsi) {
            // MSI vector: effectively always unmasked at the APIC level,
            // managed at the PCI device level.
            Ok(())
        } else {
            error!(
                "cannot find IOAPIC for global system interrupt number {}",
                gsi
            );
            Err(DeviceError::InvalidParam)
        }
    }

    fn unmask(&self, gsi: usize) -> DeviceResult {
        if let Some(apic) = self.ioapic_list.find(gsi as _) {
            apic.toggle(gsi as _, true);
            Ok(())
        } else if (X86_INT_BASE..X86_INT_LOCAL_APIC_BASE).contains(&gsi) {
            // MSI vector
            Ok(())
        } else {
            error!(
                "cannot find IOAPIC for global system interrupt number {}",
                gsi
            );
            Err(DeviceError::InvalidParam)
        }
    }

    fn configure(&self, gsi: usize, tm: IrqTriggerMode, pol: IrqPolarity) -> DeviceResult {
        let gsi = gsi as u32;
        self.with_ioapic(gsi, |apic| {
            apic.configure(gsi, tm, pol, LocalApic::bsp_id(), 0);
            Ok(())
        })
    }

    fn register_handler(&self, gsi: usize, handler: IrqHandler) -> DeviceResult {
        let gsi32 = gsi as u32;
        if self.ioapic_list.find(gsi32).is_some() {
            // Interrupción gestionada por IOAPIC (IRQ legacy/PCI-INTx).
            self.with_ioapic(gsi32, |apic| {
                let vector = apic.get_vector(gsi32) as _;
                let vector = self
                    .manager_ioapic
                    .lock()
                    .register_handler(vector, handler)? as u8;
                apic.map_vector(gsi32, vector);
                Ok(())
            })
        } else {
            // No hay IOAPIC para este GSI → es un vector MSI.
            // El hardware escribe el vector directamente en el LAPIC,
            // así que registramos el handler en manager_ioapic.table[gsi]
            // sin pasar por el IOAPIC.
            self.manager_ioapic
                .lock()
                .register_handler(gsi, handler)
                .map(|_| ())
        }
    }

    fn unregister(&self, gsi: usize) -> DeviceResult {
        let gsi = gsi as u32;
        self.with_ioapic(gsi, |apic| {
            let vector = apic.get_vector(gsi) as _;
            self.manager_ioapic.lock().unregister_handler(vector)?;
            apic.map_vector(gsi, 0);
            Ok(())
        })
    }

    fn msi_alloc_block(&self, requested_irqs: usize) -> DeviceResult<Range<usize>> {
        let alloc_size = requested_irqs.next_power_of_two();
        let start = self.manager_ioapic.lock().alloc_block(alloc_size)?;
        Ok(start..start + alloc_size)
    }

    fn msi_free_block(&self, block: Range<usize>) -> DeviceResult {
        // Must mirror `msi_alloc_block`, which allocates from `manager_ioapic`;
        // freeing to `manager_lapic` leaked the IOAPIC vectors and corrupted the
        // 16-entry LAPIC allocator.
        self.manager_ioapic
            .lock()
            .free_block(block.start, block.len())
    }

    fn msi_register_handler(
        &self,
        block: Range<usize>,
        msi_id: usize,
        handler: IrqHandler,
    ) -> DeviceResult {
        if msi_id < block.len() {
            self.manager_ioapic
                .lock()
                .overwrite_handler(block.start + msi_id, handler)
        } else {
            Err(DeviceError::InvalidParam)
        }
    }

    fn apic_timer_enable(&self) {
        if LocalApic::is_initialized() {
            // SAFETY: this will called only once for every core
            Apic::local_apic().enable_timer();
        }
    }
}
