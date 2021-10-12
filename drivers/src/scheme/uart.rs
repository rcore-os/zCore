use super::Scheme;
use crate::{utils::EventHandler, DeviceResult};

pub trait UartScheme: Scheme {
    fn try_recv(&self) -> DeviceResult<Option<u8>>;
    fn send(&self, ch: u8) -> DeviceResult;
    fn write_str(&self, s: &str) -> DeviceResult {
        for c in s.bytes() {
            self.send(c)?;
        }
        Ok(())
    }

    fn subscribe(&self, _handler: EventHandler, _once: bool) {
        unimplemented!()
    }
}
