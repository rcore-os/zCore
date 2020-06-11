#![allow(dead_code)]
use crate::get_acpi_table;
pub use acpi::{
    interrupt::{InterruptModel, InterruptSourceOverride, IoApic, Polarity, TriggerMode},
    Acpi,
};
use alloc::vec::Vec;
use lazy_static::*;
use spin::Mutex;
pub struct AcpiTable {
    inner: Acpi,
}

lazy_static! {
    static ref ACPI_TABLE: Mutex<Option<AcpiTable>> = Mutex::default();
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
