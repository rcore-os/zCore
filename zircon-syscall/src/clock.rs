use {
    super::*,
    core::sync::atomic::{AtomicU64, Ordering},
    kernel_hal::timer_now,
};

static UTC_OFFSET: AtomicU64 = AtomicU64::new(0);

const ZX_CLOCK_MONOTONIC: u32 = 0;
const ZX_CLOCK_UTC: u32 = 1;
const ZX_CLOCK_THREAD: u32 = 2;

impl Syscall<'_> {
    pub fn sys_clock_get(&self, clock_id: u32, mut time: UserOutPtr<u64>) -> ZxResult<usize> {
        match clock_id {
            ZX_CLOCK_MONOTONIC => {
                time.write(timer_now().as_secs())?;
                Ok(0)
            }
            ZX_CLOCK_UTC => {
                time.write(timer_now().as_secs() + UTC_OFFSET.load(Ordering::Relaxed))?;
                Ok(0)
            }
            ZX_CLOCK_THREAD => unimplemented!(),
            _ => Err(ZxError::NOT_SUPPORTED),
        }
    }
}
