use alloc::boxed::Box;
use bitmap_allocator::{BitAlloc, BitAlloc256};

use super::arch::interrupt::{IRQ_MAX_ID, IRQ_MIN_ID};
use crate::{HalError, HalResult};

pub use super::arch::interrupt::*;

pub type IrqHandler = Box<dyn Fn() + Send + Sync>;

const IRQ_COUNT: usize = IRQ_MAX_ID as usize + 1;

pub struct IrqManager {
    table: [Option<IrqHandler>; IRQ_COUNT],
    allocator: BitAlloc256,
}

impl IrqManager {
    pub fn alloc_block(&mut self, count: u32) -> HalResult<u32> {
        debug_assert!(count.is_power_of_two());
        let align_log2 = 31 - count.leading_zeros();
        self.allocator
            .alloc_contiguous(count as usize, align_log2 as usize)
            .map(|start| start as u32)
            .ok_or(HalError)
    }

    pub fn free_block(&mut self, start: u32, count: u32) -> HalResult {
        self.allocator
            .insert(start as usize..(start + count) as usize);
        Ok(())
    }

    /// Add a handler to IRQ table. Return the specified irq or an allocated irq on success
    pub fn add_handler(&mut self, vector: u32, handler: IrqHandler) -> HalResult<u32> {
        info!("IRQ add handler {:#x?}", vector);
        let vector = if vector < IRQ_MIN_ID {
            // allocate a valid irq number
            self.alloc_block(1)?
        } else if self.allocator.test(vector as usize) {
            self.allocator.remove(vector as usize..vector as usize + 1);
            vector
        } else {
            return Err(HalError);
        };
        self.table[vector as usize] = Some(handler);
        Ok(vector)
    }

    pub fn remove_handler(&mut self, vector: u32) -> HalResult {
        info!("IRQ remove handler {:#x?}", vector);
        if self.allocator.test(vector as usize) {
            Err(HalError)
        } else {
            self.free_block(vector, 1)?;
            self.table[vector as usize] = None;
            Ok(())
        }
    }

    pub fn overwrite_handler(&mut self, vector: u32, handler: IrqHandler) -> HalResult {
        info!("IRQ overwrite handle {:#x?}", vector);
        if self.allocator.test(vector as usize) {
            Err(HalError)
        } else {
            self.table[vector as usize] = Some(handler);
            Ok(())
        }
    }

    pub fn handle(&self, vector: u32) {
        match &self.table[vector as usize] {
            Some(f) => f(),
            None => panic!("unhandled external IRQ number: {}", vector),
        }
    }
}

impl Default for IrqManager {
    fn default() -> Self {
        const EMPTY_HANDLER: Option<Box<dyn Fn() + Send + Sync>> = None;
        let mut allocator = BitAlloc256::DEFAULT;
        allocator.insert(IRQ_MIN_ID as usize..IRQ_MAX_ID as usize + 1);
        Self {
            table: [EMPTY_HANDLER; IRQ_COUNT],
            allocator,
        }
    }
}
