use {
    super::*,
    alloc::sync::Arc,
    core::sync::atomic::{AtomicI64, Ordering},
};

/// A signed 64-bit kernel counter.
pub struct Counter {
    base: KObjectBase,
    value: AtomicI64,
}

impl_kobject!(Counter
    fn allowed_signals(&self) -> Signal {
        Signal::USER_ALL | Signal::SIGNALED | Signal::COUNTER_POSITIVE | Signal::COUNTER_NON_POSITIVE
    }
);

impl Counter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            base: KObjectBase::with_signal(Signal::COUNTER_NON_POSITIVE),
            value: AtomicI64::new(0),
        })
    }

    pub fn read(&self) -> i64 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn write(&self, value: i64) {
        let old = self.value.swap(value, Ordering::Relaxed);
        self.update_signals(old, value);
    }

    pub fn add(&self, amount: i64) -> ZxResult {
        let mut old = self.value.load(Ordering::Relaxed);
        loop {
            let new = old.checked_add(amount).ok_or(ZxError::OUT_OF_RANGE)?;
            match self
                .value
                .compare_exchange_weak(old, new, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => {
                    self.update_signals(old, new);
                    return Ok(());
                }
                Err(value) => old = value,
            }
        }
    }

    fn update_signals(&self, old: i64, new: i64) {
        if old <= 0 && new > 0 {
            self.base
                .signal_change(Signal::COUNTER_NON_POSITIVE, Signal::COUNTER_POSITIVE);
        } else if old > 0 && new <= 0 {
            self.base
                .signal_change(Signal::COUNTER_POSITIVE, Signal::COUNTER_NON_POSITIVE);
        }
    }
}
