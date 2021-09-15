use super::Scheme;
use crate::DeviceResult;

pub trait UartScheme: Scheme {
    fn try_recv(&mut self) -> DeviceResult<Option<u8>>;
    fn send(&mut self, ch: u8) -> DeviceResult;
}
