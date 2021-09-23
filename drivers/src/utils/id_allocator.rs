use alloc::boxed::Box;
use core::ops::{Deref, DerefMut, Range};

use bitmap_allocator::{BitAlloc, BitAlloc16, BitAlloc256, BitAlloc4K, BitAlloc64K};

use crate::{DeviceError, DeviceResult};

pub trait IdAllocatorWrapper: Send + Sync {
    fn new(range: Range<usize>) -> Self
    where
        Self: Sized;
    fn alloc(&mut self) -> DeviceResult<usize>;
    fn alloc_fixed(&mut self, id: usize) -> DeviceResult;
    fn alloc_contiguous(&mut self, count: usize, align_log2: usize) -> DeviceResult<usize>;
    fn free(&mut self, start_id: usize, count: usize) -> DeviceResult;
    fn is_alloced(&self, id: usize) -> bool;
}

pub struct IdAllocator(Box<dyn IdAllocatorWrapper>);

impl IdAllocator {
    pub fn new(range: Range<usize>) -> DeviceResult<Self> {
        Ok(match range.end {
            0..=0x10 => Self(Box::new(IdAllocator16::new(range))),
            0x11..=0x100 => Self(Box::new(IdAllocator256::new(range))),
            0x101..=0x1000 => Self(Box::new(IdAllocator4K::new(range))),
            0x1001..=0x10000 => Self(Box::new(IdAllocator64K::new(range))),
            _ => {
                warn!("out of range in IdAllocator::new(): {:#x?}", range);
                return Err(DeviceError::InvalidParam);
            }
        })
    }
}

impl Deref for IdAllocator {
    type Target = Box<dyn IdAllocatorWrapper>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for IdAllocator {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

macro_rules! define_allocator {
    ($name: ident, $inner: ty) => {
        struct $name($inner);

        impl IdAllocatorWrapper for $name {
            fn new(range: Range<usize>) -> Self {
                let mut inner = <$inner>::DEFAULT;
                inner.insert(range);
                Self(inner)
            }

            fn alloc(&mut self) -> DeviceResult<usize> {
                self.0.alloc().ok_or(DeviceError::NoResources)
            }

            fn alloc_fixed(&mut self, id: usize) -> DeviceResult {
                if self.0.test(id) {
                    self.0.remove(id..id + 1);
                    Ok(())
                } else {
                    Err(DeviceError::AlreadyExists)
                }
            }

            fn alloc_contiguous(&mut self, count: usize, align_log2: usize) -> DeviceResult<usize> {
                self.0
                    .alloc_contiguous(count, align_log2)
                    .ok_or(DeviceError::InvalidParam)
            }

            fn free(&mut self, start_id: usize, count: usize) -> DeviceResult {
                if count == 0 {
                    Err(DeviceError::InvalidParam)
                } else if count == 1 {
                    if !self.is_alloced(start_id) {
                        Err(DeviceError::InvalidParam)
                    } else {
                        self.0.dealloc(start_id);
                        Ok(())
                    }
                } else {
                    self.0.insert(start_id..start_id + count);
                    Ok(())
                }
            }

            fn is_alloced(&self, id: usize) -> bool {
                !self.0.test(id)
            }
        }
    };
}

define_allocator!(IdAllocator16, BitAlloc16);
define_allocator!(IdAllocator256, BitAlloc256);
define_allocator!(IdAllocator4K, BitAlloc4K);
define_allocator!(IdAllocator64K, BitAlloc64K);
