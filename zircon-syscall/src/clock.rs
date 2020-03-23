use {
    super::*,
    core::sync::atomic::{AtomicU64, Ordering},
    kernel_hal::timer_now,
    zircon_object::resource::*,
};

static UTC_OFFSET: AtomicU64 = AtomicU64::new(0);

const ZX_CLOCK_MONOTONIC: u32 = 0;
const ZX_CLOCK_UTC: u32 = 1;
const ZX_CLOCK_THREAD: u32 = 2;

impl Syscall<'_> {
    pub fn sys_clock_get(&self, clock_id: u32, mut time: UserOutPtr<u64>) -> ZxResult<usize> {
        info!("clock.get: id={}", clock_id);
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

    pub fn sys_clock_adjust(
        &self,
        hrsrc: HandleValue,
        clock_id: u32,
        offset: u64,
    ) -> ZxResult<usize> {
        info!(
            "clock.adjust: hrsrc={:?}, id={}, offset={:#x}",
            hrsrc, clock_id, offset
        );
        self.thread
            .proc()
            .validate_resource(hrsrc, ResourceKind::ROOT)?;
        match clock_id {
            ZX_CLOCK_MONOTONIC => Err(ZxError::ACCESS_DENIED),
            ZX_CLOCK_UTC => {
                UTC_OFFSET.store(offset, Ordering::Relaxed);
                Ok(0)
            }
            _ => Err(ZxError::INVALID_ARGS),
        }
    }
}
