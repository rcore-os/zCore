#![allow(dead_code)]

use core::ops::Range;

use super::IdAllocator;
use crate::{scheme::IrqHandler, DeviceError, DeviceResult};

pub struct IrqManager<const IRQ_COUNT: usize> {
    irq_range: Range<usize>,
    table: [Option<IrqHandler>; IRQ_COUNT],
    allocator: IdAllocator,
}

impl<const IRQ_COUNT: usize> IrqManager<IRQ_COUNT> {
    pub fn new(irq_range: Range<usize>) -> Self {
        assert!(irq_range.end <= IRQ_COUNT);
        const EMPTY_HANDLER: Option<IrqHandler> = None;
        let allocator = IdAllocator::new(irq_range.clone()).unwrap();
        Self {
            irq_range,
            table: [EMPTY_HANDLER; IRQ_COUNT],
            allocator,
        }
    }

    pub fn alloc_block(&mut self, count: usize) -> DeviceResult<usize> {
        debug_assert!(count.is_power_of_two());
        let align_log2 = 31 - count.leading_zeros();
        self.allocator.alloc_contiguous(count, align_log2 as _)
    }

    pub fn free_block(&mut self, start: usize, count: usize) -> DeviceResult {
        self.allocator.free(start, count)
    }

    /// Add a handler to IRQ table. Return the specified irq or an allocated irq on success
    pub fn register_handler(&mut self, irq_num: usize, handler: IrqHandler) -> DeviceResult<usize> {
        info!("IRQ add handler {:#x?}", irq_num);
        let irq_num = if !self.irq_range.contains(&irq_num) {
            // allocate a valid irq number
            self.allocator.alloc()?
        } else {
            self.allocator.alloc_fixed(irq_num)?;
            irq_num
        };
        self.table[irq_num] = Some(handler);
        Ok(irq_num)
    }

    pub fn unregister_handler(&mut self, irq_num: usize) -> DeviceResult {
        info!("IRQ remove handler {:#x?}", irq_num);
        if !self.allocator.is_alloced(irq_num) {
            Err(DeviceError::InvalidParam)
        } else {
            self.allocator.free(irq_num, 1)?;
            self.table[irq_num] = None;
            Ok(())
        }
    }

    pub fn overwrite_handler(&mut self, irq_num: usize, handler: IrqHandler) -> DeviceResult {
        info!("IRQ overwrite handle {:#x?}", irq_num);
        if !self.allocator.is_alloced(irq_num) {
            Err(DeviceError::InvalidParam)
        } else {
            self.table[irq_num] = Some(handler);
            Ok(())
        }
    }

    pub fn handle(&self, irq_num: usize) -> DeviceResult {
        if let Some(f) = &self.table[irq_num] {
            f(irq_num);
            Ok(())
        } else {
            Err(DeviceError::InvalidParam)
        }
    }
}
