use super::{event::EventScheme, Scheme};
use crate::DeviceResult;

pub trait UartScheme: Scheme + EventScheme<Event = ()> {
    fn try_recv(&self) -> DeviceResult<Option<u8>>;
    fn send(&self, ch: u8) -> DeviceResult;
    fn write_str(&self, s: &str) -> DeviceResult {
        for c in s.bytes() {
            self.send(c)?;
        }
        Ok(())
    }
}
