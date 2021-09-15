use super::Scheme;
use crate::DeviceResult;

pub trait BlockScheme: Scheme {
    fn read_block(&mut self, block_id: usize, buf: &mut [u8]) -> DeviceResult;
    fn write_block(&mut self, block_id: usize, buf: &[u8]) -> DeviceResult;
    fn flush(&mut self) -> DeviceResult;
}
