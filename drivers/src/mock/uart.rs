use std::io::{self, Read};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;

use crate::scheme::{Scheme, UartScheme};
use crate::DeviceResult;

pub struct MockUart {
    stdin_channel: Mutex<Receiver<u8>>,
}

impl MockUart {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || loop {
            let mut buf = [0];
            io::stdin().read_exact(&mut buf).unwrap();
            if tx.send(buf[0]).is_err() {
                break;
            }
            core::hint::spin_loop();
        });
        Self {
            stdin_channel: Mutex::new(rx),
        }
    }
}

impl Default for MockUart {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheme for MockUart {}

impl UartScheme for MockUart {
    fn try_recv(&self) -> DeviceResult<Option<u8>> {
        match self.stdin_channel.lock().unwrap().try_recv() {
            Ok(ch) => Ok(Some(ch)),
            _ => Ok(None),
        }
    }

    fn send(&self, ch: u8) -> DeviceResult {
        eprint!("{}", ch as char);
        Ok(())
    }

    fn write_str(&self, s: &str) -> DeviceResult {
        eprint!("{}", s);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_mock_uart() {
        let uart = MockUart::new();
        uart.write_str("Hello, World!\n").unwrap();
        uart.write_str(format!("{} + {} = {}\n", 1, 2, 1 + 2).as_str())
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Some(ch) = uart.try_recv().unwrap() {
            uart.write_str(format!("received data: {:?}({:#x})\n", ch as char, ch).as_str())
                .unwrap();
        } else {
            uart.write_str("no data to receive\n").unwrap();
        }
    }
}
