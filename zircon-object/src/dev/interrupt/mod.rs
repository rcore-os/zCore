use {
    self::event_interrupt::*,
    self::pci_interrupt::*,
    self::virtual_interrupt::*,
    crate::dev::pci::IPciNode,
    crate::object::*,
    crate::signal::*,
    alloc::{boxed::Box, sync::Arc},
    bitflags::bitflags,
    lock::Mutex,
};

mod event_interrupt;
mod pci_interrupt;
mod virtual_interrupt;

trait InterruptTrait: Sync + Send {
    /// Mask the interrupt.
    fn mask(&self);
    /// Unmask the interrupt.
    fn unmask(&self);
    /// Register the interrupt to the given handler.
    fn register_handler(&self, handler: Box<dyn Fn() + Send + Sync>) -> ZxResult;
    /// Unregister the interrupt to the given handler.
    fn unregister_handler(&self) -> ZxResult;
}

impl_kobject!(Interrupt);

/// Interrupts - Usermode I/O interrupt delivery.
///
/// ## SYNOPSIS
///
/// Interrupt objects allow userspace to create, signal, and wait on hardware interrupts.
pub struct Interrupt {
    base: KObjectBase,
    has_vcpu: bool,
    flags: InterruptFlags,
    inner: Mutex<InterruptInner>,
    trait_: Box<dyn InterruptTrait>,
}

#[derive(Default)]
struct InterruptInner {
    state: InterruptState,
    port: Option<Arc<Port>>,
    key: u64,
    timestamp: i64,
    defer_unmask: bool,
    packet_id: u64,
}

impl Drop for Interrupt {
    fn drop(&mut self) {
        self.destroy().unwrap();
    }
}

impl Interrupt {
    /// Create a new virtual interrupt.
    pub fn new_virtual() -> Arc<Self> {
        Arc::new(Interrupt {
            base: KObjectBase::new(),
            has_vcpu: false,
            flags: InterruptFlags::VIRTUAL,
            inner: Default::default(),
            trait_: VirtualInterrupt::new(),
        })
    }

    /// Create a new physical interrupt.
    pub fn new_physical(vector: usize, options: InterruptOptions) -> ZxResult<Arc<Self>> {
        let mode = options.to_mode();
        if mode != InterruptOptions::MODE_DEFAULT && mode != InterruptOptions::MODE_EDGE_HIGH {
            unimplemented!();
        }
        if options.contains(InterruptOptions::REMAP_IRQ) {
            warn!("Skip Interrupt.Remap");
        }
        let interrupt = Arc::new(Interrupt {
            base: KObjectBase::new(),
            has_vcpu: false,
            flags: InterruptFlags::empty(),
            inner: Default::default(),
            trait_: EventInterrupt::new(vector),
        });
        let interrupt_clone = interrupt.clone();
        interrupt
            .trait_
            .register_handler(Box::new(move || interrupt_clone.handle_interrupt()))?;
        interrupt.trait_.unmask();
        Ok(interrupt)
    }

    /// Create a new PCI interrupt.
    pub fn new_pci(device: Arc<dyn IPciNode>, vector: u32, maskable: bool) -> ZxResult<Arc<Self>> {
        let interrupt = Arc::new(Interrupt {
            base: KObjectBase::new(),
            has_vcpu: false,
            flags: InterruptFlags::UNMASK_PREWAIT_UNLOCKED,
            inner: Default::default(),
            trait_: PciInterrupt::new(device, vector, maskable),
        });
        let interrupt_clone = interrupt.clone();
        interrupt
            .trait_
            .register_handler(Box::new(move || interrupt_clone.handle_interrupt()))?;
        interrupt.trait_.unmask();
        Ok(interrupt)
    }

    /// Bind the interrupt object to a port.
    pub fn bind(&self, port: &Arc<Port>, key: u64) -> ZxResult {
        let mut inner = self.inner.lock();
        match inner.state {
            InterruptState::Destroy => return Err(ZxError::CANCELED),
            InterruptState::Waiting => return Err(ZxError::BAD_STATE),
            _ => (),
        }
        if inner.port.is_some() || self.has_vcpu {
            return Err(ZxError::ALREADY_BOUND);
        }
        if self
            .flags
            .contains(InterruptFlags::UNMASK_PREWAIT_UNLOCKED | InterruptFlags::MASK_POSTWAIT)
        {
            return Err(ZxError::INVALID_ARGS);
        }
        inner.port = Some(port.clone());
        inner.key = key;
        if inner.state == InterruptState::Triggered {
            inner.packet_id = port.as_ref().push_interrupt(inner.timestamp, inner.key);
            inner.state = InterruptState::NeedAck;
        }
        Ok(())
    }

    /// Unbind the interrupt object from a port.
    ///
    /// Unbinding the port removes previously queued packets to the port.
    pub fn unbind(&self, port: &Arc<Port>) -> ZxResult {
        let mut inner = self.inner.lock();
        if inner.port.is_none() || inner.port.as_ref().unwrap().id() != port.id() {
            return Err(ZxError::NOT_FOUND);
        }
        if inner.state == InterruptState::Destroy {
            return Err(ZxError::CANCELED);
        }
        port.remove_interrupt(inner.packet_id);
        inner.port = None;
        inner.key = 0;
        Ok(())
    }

    /// Triggers a virtual interrupt object.
    pub fn trigger(&self, timestamp: i64) -> ZxResult {
        if !self.flags.contains(InterruptFlags::VIRTUAL) {
            return Err(ZxError::BAD_STATE);
        }
        let mut inner = self.inner.lock();
        if inner.timestamp == 0 {
            inner.timestamp = timestamp;
        }
        if inner.state == InterruptState::Destroy {
            return Err(ZxError::CANCELED);
        }
        if inner.state == InterruptState::NeedAck && inner.port.is_some() {
            return Ok(());
        }
        if let Some(port) = &inner.port {
            // TODO: use a function to send the package
            inner.packet_id = port.push_interrupt(timestamp, inner.key);
            if self.flags.contains(InterruptFlags::MASK_POSTWAIT) {
                self.trait_.mask();
            }
            inner.timestamp = 0;
            inner.state = InterruptState::NeedAck;
        } else {
            inner.state = InterruptState::Triggered;
            self.base.signal_set(Signal::INTERRUPT_SIGNAL);
        }
        Ok(())
    }

    /// Acknowledge the interrupt and re-arm it.
    pub fn ack(&self) -> ZxResult {
        let mut inner = self.inner.lock();
        if inner.port.is_none() {
            return Err(ZxError::BAD_STATE);
        }
        if inner.state == InterruptState::Destroy {
            return Err(ZxError::CANCELED);
        }
        if inner.state == InterruptState::NeedAck {
            if self.flags.contains(InterruptFlags::UNMASK_PREWAIT) {
                self.trait_.unmask();
            } else if self.flags.contains(InterruptFlags::UNMASK_PREWAIT_UNLOCKED) {
                inner.defer_unmask = true;
            }
            if inner.timestamp > 0 {
                // TODO: use a function to send the package
                inner.packet_id = inner
                    .port
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .push_interrupt(inner.timestamp, inner.key);
                if self.flags.contains(InterruptFlags::MASK_POSTWAIT) {
                    self.trait_.mask();
                }
                inner.timestamp = 0;
            } else {
                inner.state = InterruptState::Idle;
            }
        }
        if inner.defer_unmask {
            self.trait_.unmask();
        }
        Ok(())
    }

    /// Destroy the interrupt.
    pub fn destroy(&self) -> ZxResult {
        self.trait_.mask();
        self.trait_.unregister_handler()?;
        let mut inner = self.inner.lock();
        if let Some(port) = &inner.port {
            let in_queue = port.remove_interrupt(inner.packet_id);
            match inner.state {
                InterruptState::NeedAck => {
                    inner.state = InterruptState::Destroy;
                    if !in_queue {
                        Err(ZxError::NOT_FOUND)
                    } else {
                        Ok(())
                    }
                }
                InterruptState::Idle => {
                    inner.state = InterruptState::Destroy;
                    Ok(())
                }
                _ => Ok(()),
            }
        } else {
            inner.state = InterruptState::Destroy;
            self.base.signal_set(Signal::INTERRUPT_SIGNAL);
            Ok(())
        }
    }

    /// Wait until the interrupt is triggered.
    pub async fn wait(self: &Arc<Self>) -> ZxResult<i64> {
        let mut defer_unmask = false;
        let object = self.clone() as Arc<dyn KernelObject>;
        loop {
            {
                let mut inner = self.inner.lock();
                if inner.port.is_some() || self.has_vcpu {
                    return Err(ZxError::BAD_STATE);
                }
                match inner.state {
                    InterruptState::Destroy => return Err(ZxError::CANCELED),
                    InterruptState::Triggered => {
                        inner.state = InterruptState::NeedAck;
                        let timestamp = inner.timestamp;
                        inner.timestamp = 0;
                        self.base.signal_clear(Signal::INTERRUPT_SIGNAL);
                        return Ok(timestamp);
                    }
                    InterruptState::NeedAck => {
                        if self.flags.contains(InterruptFlags::UNMASK_PREWAIT) {
                            self.trait_.unmask();
                        } else if self.flags.contains(InterruptFlags::UNMASK_PREWAIT_UNLOCKED) {
                            defer_unmask = true;
                        }
                    }
                    InterruptState::Idle => (),
                    _ => return Err(ZxError::BAD_STATE),
                }
                inner.state = InterruptState::Waiting;
            }
            if defer_unmask {
                self.trait_.unmask();
            }
            object.wait_signal(Signal::INTERRUPT_SIGNAL).await;
        }
    }

    fn handle_interrupt(&self) {
        let mut inner = self.inner.lock();
        if self.flags.contains(InterruptFlags::MASK_POSTWAIT) {
            self.trait_.mask();
        }
        if inner.timestamp == 0 {
            // Not sure ZX_CLOCK_MONOTONIC or ZX_CLOCK_UTC
            inner.timestamp = kernel_hal::timer::timer_now().as_nanos() as i64;
        }
        match &inner.port {
            Some(port) => {
                if inner.state != InterruptState::NeedAck {
                    // TODO: use a function to send the package
                    inner.packet_id = port.as_ref().push_interrupt(inner.timestamp, inner.key);
                    if self.flags.contains(InterruptFlags::MASK_POSTWAIT) {
                        self.trait_.mask();
                    }
                    inner.timestamp = 0;

                    inner.state = InterruptState::NeedAck;
                }
            }
            None => {
                self.base.signal_set(Signal::INTERRUPT_SIGNAL);
                inner.state = InterruptState::Triggered;
            }
        }
    }
}

#[derive(PartialEq, Debug)]
enum InterruptState {
    Waiting = 0,
    Destroy = 1,
    Triggered = 2,
    NeedAck = 3,
    Idle = 4,
}

impl Default for InterruptState {
    fn default() -> Self {
        InterruptState::Idle
    }
}

bitflags! {
    /// Bits for Interrupt.flags.
    pub struct InterruptFlags: u32 {
        #[allow(clippy::identity_op)]
        /// The interrupt is virtual.
        const VIRTUAL                  = 1 << 0;
        /// The interrupt should be unmasked before waiting on the event.
        const UNMASK_PREWAIT           = 1 << 1;
        /// The same as **INTERRUPT_UNMASK_PREWAIT** except release the dispatcher
        /// spinlock before waiting.
        const UNMASK_PREWAIT_UNLOCKED  = 1 << 2;
        /// The interrupt should be masked following waiting.
        const MASK_POSTWAIT            = 1 << 4;
    }
}

bitflags! {
    /// Interrupt bind flags.
    pub struct InterruptOptions: u32 {
        #[allow(clippy::identity_op)]
        /// Remap interrupt request(IRQ).
        const REMAP_IRQ = 0x1;
        /// Default mode.
        const MODE_DEFAULT = 0 << 1;
        /// Falling edge triggered.
        const MODE_EDGE_LOW = 1 << 1;
        /// Rising edge triggered.
        const MODE_EDGE_HIGH = 2 << 1;
        /// Low level triggered.
        const MODE_LEVEL_LOW = 3 << 1;
        /// High level triggered.
        const MODE_LEVEL_HIGH = 4 << 1;
        /// Falling/rising edge triggered.
        const MODE_EDGE_BOTH = 5 << 1;
        /// Virtual interrupt.
        const VIRTUAL = 0x10;
    }
}

impl InterruptOptions {
    /// Extract the mode bits.
    pub fn to_mode(self) -> Self {
        InterruptOptions::from_bits_truncate(0xe) & self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn bind() {
        let interrupt = Interrupt::new_virtual();
        let port = Port::new(1).unwrap();
        assert_eq!(interrupt.unbind(&port).unwrap_err(), ZxError::NOT_FOUND);
        assert!(interrupt.bind(&port, 1).is_ok());

        assert!(interrupt.destroy().is_ok());
        assert_eq!(interrupt.unbind(&port).unwrap_err(), ZxError::CANCELED);

        let interrupt = Interrupt::new_virtual();
        assert_eq!(interrupt.unbind(&port).unwrap_err(), ZxError::NOT_FOUND);
        assert!(interrupt.bind(&port, 1).is_ok());

        assert!(interrupt.trigger(1234).is_ok());
        let packet = port.wait().await;
        assert_eq!(
            PortPacketRepr::from(&packet),
            PortPacketRepr {
                key: 1,
                status: ZxError::OK,
                data: PayloadRepr::Interrupt(PacketInterrupt {
                    timestamp: 1234,
                    _reserved0: 0,
                    _reserved1: 0,
                    _reserved2: 0,
                }),
            }
        );
        assert!(interrupt.unbind(&port).is_ok());
    }
}
