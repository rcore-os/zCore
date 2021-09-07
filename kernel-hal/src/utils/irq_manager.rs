#![allow(dead_code)]

use alloc::boxed::Box;
use bitmap_allocator::{BitAlloc, BitAlloc256};
use core::ops::Range;

use crate::{HalError, HalResult};

pub type IrqHandler = Box<dyn Fn() + Send + Sync>;

const IRQ_COUNT: usize = 256;

pub struct IrqManager {
    irq_range: Range<u32>,
    table: [Option<IrqHandler>; IRQ_COUNT],
    allocator: BitAlloc256,
}

impl IrqManager {
    pub fn new(irq_min_id: u32, irq_max_id: u32) -> Self {
        const EMPTY_HANDLER: Option<Box<dyn Fn() + Send + Sync>> = None;
        let mut allocator = BitAlloc256::DEFAULT;
        allocator.insert(irq_min_id as usize..irq_max_id as usize + 1);
        Self {
            irq_range: irq_min_id..irq_max_id + 1,
            table: [EMPTY_HANDLER; IRQ_COUNT],
            allocator,
        }
    }

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
    pub fn register_handler(&mut self, vector: u32, handler: IrqHandler) -> HalResult<u32> {
        info!("IRQ add handler {:#x?}", vector);
        let vector = if !self.irq_range.contains(&vector) {
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

    pub fn unregister_handler(&mut self, vector: u32) -> HalResult {
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
