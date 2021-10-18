#![allow(missing_docs)]
//! `sys_pci_init` args.
//!
//! reference: zircon/system/public/zircon/syscalls/pci.h

use kernel_hal::drivers::prelude::{IrqPolarity, IrqTriggerMode};
use kernel_hal::interrupt;

use super::constants::*;
use crate::{ZxError, ZxResult};

#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct PciIrqSwizzleLut(
    [[[u32; PCI_MAX_LEGACY_IRQ_PINS]; PCI_MAX_FUNCTIONS_PER_DEVICE]; PCI_MAX_DEVICES_PER_BUS],
);

#[repr(C)]
#[derive(Debug)]
pub struct PciInitArgsIrqs {
    pub global_irq: u32,
    pub level_triggered: bool,
    pub active_high: bool,
    pub padding1: [u8; 2],
}

#[repr(C)]
#[derive(Debug)]
pub struct PciInitArgsHeader {
    pub dev_pin_to_global_irq: PciIrqSwizzleLut,
    pub num_irqs: u32,
    pub irqs: [PciInitArgsIrqs; PCI_MAX_IRQS],
    pub addr_window_count: u32,
}

#[repr(C)]
#[derive(Debug)]
pub struct PciInitArgsAddrWindows {
    pub base: u64,
    pub size: usize,
    pub bus_start: u8,
    pub bus_end: u8,
    pub cfg_space_type: u8,
    pub has_ecam: bool,
    pub padding1: [u8; 4],
}

pub const PCI_INIT_ARG_MAX_ECAM_WINDOWS: usize = 2;
pub const PCI_INIT_ARG_MAX_SIZE: usize = core::mem::size_of::<PciInitArgsAddrWindows>()
    * PCI_INIT_ARG_MAX_ECAM_WINDOWS
    + core::mem::size_of::<PciInitArgsHeader>();

impl PciInitArgsHeader {
    pub fn configure_interrupt(&mut self) -> ZxResult {
        for i in 0..self.num_irqs as usize {
            let irq = &mut self.irqs[i];
            let global_irq = irq.global_irq;
            if !interrupt::is_valid_irq(global_irq as usize) {
                irq.global_irq = PCI_NO_IRQ_MAPPING;
                self.dev_pin_to_global_irq.remove_irq(global_irq);
            } else {
                let tm = if irq.level_triggered {
                    IrqTriggerMode::Level
                } else {
                    IrqTriggerMode::Edge
                };
                let pol = if irq.active_high {
                    IrqPolarity::ActiveHigh
                } else {
                    IrqPolarity::ActiveLow
                };
                interrupt::configure_irq(global_irq as usize, tm, pol)
                    .map_err(|_| ZxError::INVALID_ARGS)?;
            }
        }
        Ok(())
    }
}

impl PciIrqSwizzleLut {
    pub(super) fn swizzle(&self, dev_id: usize, func_id: usize, pin: usize) -> ZxResult<usize> {
        if dev_id >= PCI_MAX_DEVICES_PER_BUS
            || func_id >= PCI_MAX_FUNCTIONS_PER_DEVICE
            || pin >= PCI_MAX_LEGACY_IRQ_PINS
        {
            return Err(ZxError::INVALID_ARGS);
        }
        let irq = self.0[dev_id][func_id][pin];
        if irq == PCI_NO_IRQ_MAPPING {
            Err(ZxError::NOT_FOUND)
        } else {
            Ok(irq as usize)
        }
    }

    fn remove_irq(&mut self, irq: u32) {
        for dev in self.0.iter_mut() {
            for func in dev.iter_mut() {
                for pin in func.iter_mut() {
                    if *pin == irq {
                        *pin = PCI_NO_IRQ_MAPPING;
                    }
                }
            }
        }
    }
}
