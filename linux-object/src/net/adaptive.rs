//! Pure helpers for NIC wait-loop tuning (unit-testable).

pub(crate) const NET_POLL_INTERVAL_MIN_US: u64 = 4_000;
pub(crate) const NET_POLL_INTERVAL_BASE_US: u64 = 16_000;
pub(crate) const NET_POLL_INTERVAL_MAX_US: u64 = 32_000;
pub(crate) const DEFERRED_NET_JOBS_PER_TICK_BASE: usize = 4;
pub(crate) const DEFERRED_NET_JOBS_PER_TICK_MAX: usize = 12;

/// Deferred IRQ jobs to run before a NIC poll, scaled by queue depth.
#[inline]
pub(crate) fn deferred_jobs_budget(pending: usize) -> usize {
    DEFERRED_NET_JOBS_PER_TICK_BASE
        + pending
            .min(DEFERRED_NET_JOBS_PER_TICK_MAX.saturating_sub(DEFERRED_NET_JOBS_PER_TICK_BASE))
}

/// Full [`super::poll_ifaces`] interval for multiplex wait loops.
#[inline]
pub(crate) fn net_poll_interval_us(pending: usize) -> u64 {
    if pending >= 8 {
        NET_POLL_INTERVAL_MIN_US
    } else if pending == 0 {
        NET_POLL_INTERVAL_MAX_US
    } else {
        NET_POLL_INTERVAL_BASE_US
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_jobs_scales_with_backlog() {
        assert_eq!(deferred_jobs_budget(0), 4);
        assert_eq!(deferred_jobs_budget(4), 8);
        assert_eq!(deferred_jobs_budget(100), 12);
    }

    #[test]
    fn net_poll_interval_tightens_when_busy() {
        assert_eq!(net_poll_interval_us(0), 32_000);
        assert_eq!(net_poll_interval_us(1), 16_000);
        assert_eq!(net_poll_interval_us(7), 16_000);
        assert_eq!(net_poll_interval_us(8), 4_000);
    }
}
