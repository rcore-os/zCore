use super::Scheme;
use crate::DeviceResult;

pub trait NetScheme: Scheme {
    fn recv(&mut self, buf: &mut [u8]) -> DeviceResult<usize>;
    fn send(&mut self, buf: &[u8]) -> DeviceResult<usize>;
}
