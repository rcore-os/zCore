use super::*;
use crate::object::*;
use alloc::sync::Arc;

/// Signalable event for concurrent programming
///
/// ## SYNOPSIS
///
/// Events are user-signalable objects. The 8 signal bits reserved for
/// userspace (`ZX_USER_SIGNAL_0` through `ZX_USER_SIGNAL_7`) may be set,
/// cleared, and waited upon.
pub struct Event {
    base: KObjectBase,
    _counter: CountHelper,
}

impl_kobject!(Event
    fn allowed_signals(&self) -> Signal {
        Signal::USER_ALL | Signal::SIGNALED
    }
);
define_count_helper!(Event);

impl Event {
    /// Create a new `Event`.
    pub fn new() -> Arc<Self> {
        Arc::new(Event {
            base: KObjectBase::default(),
            _counter: CountHelper::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allowed_signals() {
        let event = Event::new();
        assert!(Signal::verify_user_signal(
            event.allowed_signals(),
            (Signal::USER_SIGNAL_5 | Signal::SIGNALED).bits().into()
        )
        .is_ok());
    }
}
