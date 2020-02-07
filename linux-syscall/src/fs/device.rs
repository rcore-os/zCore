//! Implement Device

#![allow(dead_code)]

use alloc::vec::Vec;
use rcore_fs::dev::*;
use spin::RwLock;

pub struct MemBuf(RwLock<Vec<u8>>);

impl MemBuf {
    pub fn new(v: Vec<u8>) -> Self {
        MemBuf(RwLock::new(v))
    }
}

impl Device for MemBuf {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let slice = self.0.read();
        let len = buf.len().min(slice.len() - offset);
        buf[..len].copy_from_slice(&slice[offset..offset + len]);
        Ok(len)
    }
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        let mut slice = self.0.write();
        let len = buf.len().min(slice.len() - offset);
        slice[offset..offset + len].copy_from_slice(&buf[..len]);
        Ok(len)
    }
    fn sync(&self) -> Result<()> {
        Ok(())
    }
}
