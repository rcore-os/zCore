use {super::*, alloc::sync::Arc, kernel_hal::sync::Mutex};

/// A signed 64-bit kernel counter.
pub struct Counter {
    base: KObjectBase,
    value: Mutex<i64>,
}

impl_kobject!(Counter
    fn allowed_signals(&self) -> Signal {
        Signal::USER_ALL | Signal::SIGNALED
    }
);

impl Counter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            base: KObjectBase::with_signal(Signal::COUNTER_NON_POSITIVE),
            value: Mutex::new(0),
        })
    }

    pub fn read(&self) -> i64 {
        *self.value.lock()
    }

    pub fn write(&self, value: i64) {
        let mut current = self.value.lock();
        let old = *current;
        *current = value;
        self.update_signals(old, value);
    }

    pub fn add(&self, amount: i64) -> ZxResult {
        let mut current = self.value.lock();
        let old = *current;
        let new = old.checked_add(amount).ok_or(ZxError::OUT_OF_RANGE)?;
        *current = new;
        self.update_signals(old, new);
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn counter_value_and_signal_after_concurrent_writers() {
        let c = Counter::new();
        let barrier = Arc::new(Barrier::new(5));
        let workers: Vec<_> = (0..4)
            .map(|id| {
                let c = c.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    for _ in 0..10000 {
                        barrier.wait();
                        c.write(if id % 2 == 0 { 1 } else { 0 });
                        barrier.wait();
                    }
                })
            })
            .collect();
        let mut mismatch = None;
        for round in 0..10000 {
            c.write(1);
            c.write(0);
            barrier.wait();
            barrier.wait();
            let value = c.read();
            let signal = c.signal();
            if signal.contains(Signal::COUNTER_POSITIVE) != (value > 0) {
                mismatch.get_or_insert((round, value, signal));
            }
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(mismatch.is_none(), "value/signal mismatch: {:?}", mismatch);
    }

    #[test]
    fn overflow_preserves_value_and_signals() {
        let counter = Counter::new();
        counter.write(i64::MAX);
        assert_eq!(counter.add(1), Err(ZxError::OUT_OF_RANGE));
        assert_eq!(counter.read(), i64::MAX);
        assert!(counter.signal().contains(Signal::COUNTER_POSITIVE));
        assert!(!counter
            .allowed_signals()
            .intersects(Signal::COUNTER_POSITIVE | Signal::COUNTER_NON_POSITIVE));
    }
}
