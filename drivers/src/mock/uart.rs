use std::collections::VecDeque;

use async_std::{io, io::prelude::*, task};
use spin::Mutex;

use crate::scheme::{impl_event_scheme, Scheme, UartScheme};
use crate::utils::EventListener;
use crate::DeviceResult;

const UART_BUF_LEN: usize = 256;

lazy_static::lazy_static! {
    static ref UART_BUF: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::with_capacity(UART_BUF_LEN));
}

pub struct MockUart {
    listener: EventListener,
}

impl_event_scheme!(MockUart);

impl MockUart {
    pub fn new() -> Self {
        Self {
            listener: EventListener::new(),
        }
    }

    pub fn start_irq_service(irq_handler: impl Fn() + Send + Sync + 'static) {
        task::spawn(async move {
            loop {
                let mut buf = [0; UART_BUF_LEN];
                let remains = UART_BUF_LEN - UART_BUF.lock().len();
                if remains > 0 {
                    if let Ok(n) = io::stdin().read(&mut buf[..remains]).await {
                        {
                            let mut uart_buf = UART_BUF.lock();
                            for c in &buf[..n] {
                                uart_buf.push_back(*c);
                            }
                        }
                        irq_handler();
                    }
                }
                task::yield_now().await;
            }
        });
    }
}

impl Default for MockUart {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheme for MockUart {
    fn name(&self) -> &str {
        "mock-uart"
    }

    fn handle_irq(&self, _irq_num: usize) {
        self.listener.trigger(());
    }
}

impl UartScheme for MockUart {
    fn try_recv(&self) -> DeviceResult<Option<u8>> {
        if let Some(c) = UART_BUF.lock().pop_front() {
            Ok(Some(c))
        } else {
            Ok(None)
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
    use std::sync::Arc;

    #[test]
    fn test_mock_uart() {
        let uart = Arc::new(MockUart::new());
        let u = uart.clone();
        MockUart::start_irq_service(move || u.handle_irq(0));

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
