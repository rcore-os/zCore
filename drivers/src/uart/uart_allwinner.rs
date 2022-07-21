use crate::{
    scheme::{impl_event_scheme, Scheme, UartScheme},
    utils::EventListener,
    DeviceResult, VirtAddr,
};
use d1_pac::uart;
use lock::Mutex;

pub struct UartAllwinner {
    inner: Mutex<Inner>,
    listener: EventListener,
}

impl_event_scheme!(UartAllwinner);

impl UartAllwinner {
    pub fn new(base: VirtAddr) -> Self {
        let inner = Inner(base);
        inner.init();
        Self {
            inner: Mutex::new(inner),
            listener: EventListener::new(),
        }
    }
}

impl Scheme for UartAllwinner {
    #[inline]
    fn name(&self) -> &str {
        "uart-allwinner"
    }

    #[inline]
    fn handle_irq(&self, _irq_num: usize) {
        self.listener.trigger(());
    }
}

impl UartScheme for UartAllwinner {
    #[inline]
    fn try_recv(&self) -> DeviceResult<Option<u8>> {
        self.inner.lock().try_recv()
    }

    #[inline]
    fn send(&self, ch: u8) -> DeviceResult {
        self.inner.lock().send(ch)
    }

    #[inline]
    fn write_str(&self, s: &str) -> DeviceResult {
        self.inner.lock().write_str(s)
    }
}

struct Inner(VirtAddr);

impl Inner {
    /// 初始化串口控制器
    /// BAUD 115200
    /// FIFO ON
    fn init(&self) {
        let block = self.block();
        // disable interrupts
        block.ier().reset();
        // enable fifo
        block.fcr().write(|w| w.fifoe().set_bit());
        {
            block.halt.write(|w| w.halt_tx().set_bit());
            block.lcr.write(|w| w.dlab().set_bit());
            // 13 for 115200
            block.dll().write(|w| w.dll().variant(13));
            block.dlh().write(|w| w.dlh().variant(0));
            // no break | parity disabled | 1 stop bit | 8 data bits
            block.lcr.write(|w| w.dls().eight());
            #[rustfmt::skip]
            block.halt.write(|w| w
                .change_update().set_bit()
                .chcfg_at_busy().set_bit());
        }
        // reset fifo
        #[rustfmt::skip]
        block.fcr().write(|w| w
            .xfifor().set_bit()
            .rfifor().set_bit()
            .fifoe() .set_bit()
        );
        // uart mode
        block.mcr.reset();
        // enable interrupts
        block.ier().write(|w| w.erbfi().set_bit());
    }

    /// 接收
    fn try_recv(&self) -> DeviceResult<Option<u8>> {
        let block = self.block();
        if block.lsr.read().dr().bit_is_set() {
            Ok(Some(block.rbr().read().bits() as _))
        } else {
            Ok(None)
        }
    }

    /// 发送
    fn send(&self, ch: u8) -> DeviceResult {
        let block = self.block();
        // 等待 FIFO 空位
        while block.usr.read().tfnf().is_full() {
            core::hint::spin_loop();
        }
        block.thr().write(|w| w.thr().variant(ch));
        Ok(())
    }

    fn write_str(&mut self, s: &str) -> DeviceResult {
        for b in s.bytes() {
            match b {
                b'\n' => {
                    self.send(b'\r')?;
                    self.send(b'\n')?;
                }
                _ => self.send(b)?,
            }
        }
        Ok(())
    }

    #[inline]
    fn block(&self) -> &uart::RegisterBlock {
        unsafe { &*(self.0 as *const _) }
    }
}
