#![allow(dead_code)]

use alloc::vec::Vec;
use core::ptr::NonNull;

use acpi::interrupt::{InterruptModel, InterruptSourceOverride, IoApic, Polarity, TriggerMode};
use acpi::{parse_rsdp, Acpi, AcpiHandler, PhysicalMapping};
use spin::Mutex;

use super::super::mem::phys_to_virt;
use crate::PAGE_SIZE;

pub struct AcpiTable {
    inner: Acpi,
}

lazy_static::lazy_static! {
    static ref ACPI_TABLE: Mutex<Option<AcpiTable>> = Mutex::default();
}

/// Build ACPI Table
struct AcpiHelper;

impl AcpiHandler for AcpiHelper {
    unsafe fn map_physical_region<T>(
        &mut self,
        physical_address: usize,
        size: usize,
    ) -> PhysicalMapping<T> {
        #[allow(non_snake_case)]
        let OFFSET = 0;
        let page_start = physical_address / PAGE_SIZE;
        let page_end = (physical_address + size + PAGE_SIZE - 1) / PAGE_SIZE;
        PhysicalMapping::<T> {
            physical_start: physical_address,
            virtual_start: NonNull::new_unchecked(phys_to_virt(physical_address + OFFSET) as *mut T),
            mapped_length: size,
            region_length: PAGE_SIZE * (page_end - page_start),
        }
    }
    fn unmap_physical_region<T>(&mut self, _region: PhysicalMapping<T>) {}
}

fn get_acpi_table() -> Option<Acpi> {
    let mut handler = AcpiHelper;
    match unsafe {
        parse_rsdp(
            &mut handler,
            super::special::pc_firmware_tables().0 as usize,
        )
    } {
        Ok(table) => Some(table),
        Err(info) => {
            warn!("get_acpi_table error: {:#x?}", info);
            None
        }
    }
}

impl AcpiTable {
    fn initialize_check() {
        #[cfg(target_arch = "x86_64")]
        {
            let mut table = ACPI_TABLE.lock();
            if table.is_none() {
                *table = get_acpi_table().map(|x| AcpiTable { inner: x });
            }
        }
    }
    pub fn invalidate() {
        *ACPI_TABLE.lock() = None;
    }
    pub fn get_ioapic() -> Vec<IoApic> {
        Self::initialize_check();
        let table = ACPI_TABLE.lock();
        match &*table {
            None => Vec::default(),
            Some(table) => match table.inner.interrupt_model.as_ref().unwrap() {
                InterruptModel::Apic(apic) => {
                    apic.io_apics.iter().map(|x| IoApic { ..*x }).collect()
                }
                _ => Vec::default(),
            },
        }
    }
    pub fn get_interrupt_source_overrides() -> Vec<InterruptSourceOverride> {
        Self::initialize_check();
        let table = ACPI_TABLE.lock();
        match &*table {
            None => Vec::default(),
            Some(table) => match table.inner.interrupt_model.as_ref().unwrap() {
                InterruptModel::Apic(apic) => apic
                    .interrupt_source_overrides
                    .iter()
                    .map(|x| InterruptSourceOverride {
                        polarity: Self::clone_polarity(&x.polarity),
                        trigger_mode: Self::clone_trigger_mode(&x.trigger_mode),
                        ..*x
                    })
                    .collect(),
                _ => Vec::default(),
            },
        }
    }
    fn clone_polarity(x: &Polarity) -> Polarity {
        match x {
            Polarity::SameAsBus => Polarity::SameAsBus,
            Polarity::ActiveHigh => Polarity::ActiveHigh,
            Polarity::ActiveLow => Polarity::ActiveLow,
        }
    }
    fn clone_trigger_mode(x: &TriggerMode) -> TriggerMode {
        match x {
            TriggerMode::SameAsBus => TriggerMode::SameAsBus,
            TriggerMode::Edge => TriggerMode::Edge,
            TriggerMode::Level => TriggerMode::Level,
        }
    }
}
