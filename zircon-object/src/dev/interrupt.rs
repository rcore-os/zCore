#![allow(warnings)]
use {
    crate::object::*,
    alloc::sync::Arc,
    spin::Mutex,
    bitflags::bitflags,
    crate::signal::Port,
};

// Interrupt refers to virtual_interrupt in zircon
pub struct Interrupt {
    base: KObjectBase,
    flags: InterruptFlags,
    hasvcpu: bool,
    inner: Mutex<InterruptInner>,
}

struct InterruptInner {
    state: InterruptState,
    port: Option<Arc<Port>>,
}

impl_kobject!(Interrupt);

impl Interrupt {
    pub fn create() -> Arc<Self> {
        // virtual_interrupt
        Arc::new(Interrupt {
            base: KObjectBase::new(),
            flags: InterruptFlags::VIRTUAL,
            hasvcpu: false,
            inner: Mutex::new(InterruptInner {
                state: InterruptState::IDLE,
                port: Option::None,
            })
        })
    }

    pub fn bind(&self, port: Arc<Port>, key: u64) -> ZxResult {
        let mut inner = self.inner.lock();
        match inner.state {
            InterruptState::DESTORY => return Err(ZxError::CANCELED),
            InterruptState::WAITING => return Err(ZxError::BAD_STATE),
            _ => (),
        }
        if self.flags.contains(InterruptFlags::UNMASK_PREWAIT_UNLOCKED | InterruptFlags::MASK_POSTWAIT) {
            return Err(ZxError::INVALID_ARGS);
        }
        unimplemented!();
        Ok(())
    }

    pub fn unbind(&self, port: Arc<Port>) -> ZxResult {
        unimplemented!();
        Ok(())
    }
}

enum InterruptState {
    WAITING = 0,
    DESTORY = 1,
    TRIGGERED = 2,
    NEEDACK = 3,
    IDLE = 4,
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
