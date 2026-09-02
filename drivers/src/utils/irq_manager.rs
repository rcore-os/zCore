use super::IdAllocator;
use crate::{prelude::IrqHandler, DeviceError, DeviceResult};
use core::ops::Range;

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

    #[allow(unused)]
    pub fn alloc_block(&mut self, count: usize) -> DeviceResult<usize> {
        info!("IRQ alloc_block {}", count);
        debug_assert!(count.is_power_of_two());
        let align_log2 = 31 - (count as u32).leading_zeros();
        self.allocator.alloc_contiguous(count, align_log2 as _)
    }

    #[allow(unused)]
    pub fn free_block(&mut self, start: usize, count: usize) -> DeviceResult {
        info!("IRQ free_block {:#x?}", start..start + count);
        self.allocator.free(start, count)
    }

    /// Add a handler to IRQ table. if `irq_num == 0`, we need to allocate one.
    /// Returns the specified IRQ number or an allocated IRQ on success.
    pub fn register_handler(&mut self, irq_num: usize, handler: IrqHandler) -> DeviceResult<usize> {
        info!("IRQ register handler {}", irq_num);
        let irq_num = if irq_num == 0 {
            // allocate a valid IRQ number
            self.allocator.alloc()?
        } else if self.irq_range.contains(&irq_num) {
            self.allocator.alloc_fixed(irq_num)?;
            irq_num
        } else {
            return Err(DeviceError::InvalidParam);
        };
        self.table[irq_num] = Some(handler);
        Ok(irq_num)
    }

    #[cfg(not(target_arch = "aarch64"))]
    pub fn unregister_handler(&mut self, irq_num: usize) -> DeviceResult {
        info!("IRQ unregister handler {}", irq_num);
        if !self.allocator.is_alloced(irq_num) {
            Err(DeviceError::InvalidParam)
        } else {
            self.allocator.free(irq_num, 1)?;
            self.table[irq_num] = None;
            Ok(())
        }
    }

    #[allow(unused)]
    pub fn overwrite_handler(&mut self, irq_num: usize, handler: IrqHandler) -> DeviceResult {
        info!("IRQ overwrite handle {}", irq_num);
        if !self.allocator.is_alloced(irq_num) {
            Err(DeviceError::InvalidParam)
        } else {
            self.table[irq_num] = Some(handler);
            Ok(())
        }
    }

    pub fn handle(&self, irq_num: usize) -> DeviceResult {
        if let Some(f) = &self.table[irq_num] {
            f();
            Ok(())
        } else {
            Err(DeviceError::InvalidParam)
        }
    }
}
