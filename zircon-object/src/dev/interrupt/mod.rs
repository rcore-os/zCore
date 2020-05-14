use {
    self::event_interrupt::*,
    self::virtual_interrupt::*,
    crate::object::*,
    crate::signal::*,
    alloc::{boxed::Box, sync::Arc},
    bitflags::bitflags,
    spin::Mutex,
};

mod event_interrupt;
mod virtual_interrupt;

trait InterruptTrait: Sync + Send {
    fn mask(&self);
    fn unmask(&self);
    fn register_handler(&self, handler: Box<dyn Fn() + Send + Sync>) -> ZxResult;
    fn unregister_handler(&self) -> ZxResult;
}

impl_kobject!(Interrupt);

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
    pub fn new_physical(mut vector: usize, options: InterruptOptions) -> ZxResult<Arc<Self>> {
        let mode = options.to_mode();
        if mode != InterruptOptions::MODE_DEFAULT && mode != InterruptOptions::MODE_EDGE_HIGH {
            unimplemented!();
        }
        // I don't know the real mapping, +16 only to avoid conflict
        if options.contains(InterruptOptions::REMAP_IRQ) {
            vector += 16;
            // vector = EventInterrupt::remap(vector);
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
            inner.timestamp = kernel_hal::timer_now().as_nanos() as i64;
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
    pub struct InterruptFlags: u32 {
        #[allow(clippy::identity_op)]
        const VIRTUAL                  = 1 << 0;
        const UNMASK_PREWAIT           = 1 << 1;
        const UNMASK_PREWAIT_UNLOCKED  = 1 << 2;
        const MASK_POSTWAIT            = 1 << 4;
    }
}

bitflags! {
    pub struct InterruptOptions: u32 {
        #[allow(clippy::identity_op)]
        const REMAP_IRQ = 0x1;
        const MODE_DEFAULT = 0 << 1;
        const MODE_EDGE_LOW = 1 << 1;
        const MODE_EDGE_HIGH = 2 << 1;
        const MODE_LEVEL_LOW = 3 << 1;
        const MODE_LEVEL_HIGH = 4 << 1;
        const MODE_EDGE_BOTH = 5 << 1;
        const VIRTUAL = 0x10;
    }
}

impl InterruptOptions {
    pub fn to_mode(self) -> Self {
        InterruptOptions::from_bits_truncate(0xe) & self
    }
}
