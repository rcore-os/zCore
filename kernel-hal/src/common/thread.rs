use core::time::Duration;

use super::future::{SleepFuture, YieldFuture};

/// Sleeps until the specified of time.
pub async fn sleep_until(deadline: Duration) {
    SleepFuture::new(deadline).await
}

/// Yields execution back to the async runtime.
pub async fn yield_now() {
    YieldFuture::default().await
}
