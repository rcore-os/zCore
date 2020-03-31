use {
    super::*,
    core::{
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    },
    kernel_hal::{sleep_until, timer_now, yield_now},
    zircon_object::{resource::*, task::*},
};

static UTC_OFFSET: AtomicU64 = AtomicU64::new(0);

const ZX_CLOCK_MONOTONIC: u32 = 0;
const ZX_CLOCK_UTC: u32 = 1;
const ZX_CLOCK_THREAD: u32 = 2;

impl Syscall<'_> {
    pub fn sys_clock_get(&self, clock_id: u32, mut time: UserOutPtr<u64>) -> ZxResult {
        info!("clock.get: id={}", clock_id);
        match clock_id {
            ZX_CLOCK_MONOTONIC => {
                time.write(timer_now().as_nanos() as u64)?;
                Ok(())
            }
            ZX_CLOCK_UTC => {
                time.write(timer_now().as_nanos() as u64 + UTC_OFFSET.load(Ordering::Relaxed))?;
                Ok(())
            }
            ZX_CLOCK_THREAD => unimplemented!(),
            _ => Err(ZxError::NOT_SUPPORTED),
        }
    }

    pub fn sys_clock_adjust(&self, hrsrc: HandleValue, clock_id: u32, offset: u64) -> ZxResult {
        info!(
            "clock.adjust: hrsrc={:#x?}, id={:#x}, offset={:#x}",
            hrsrc, clock_id, offset
        );
        self.thread
            .proc()
            .validate_resource(hrsrc, ResourceKind::ROOT)?;
        match clock_id {
            ZX_CLOCK_MONOTONIC => Err(ZxError::ACCESS_DENIED),
            ZX_CLOCK_UTC => {
                UTC_OFFSET.store(offset, Ordering::Relaxed);
                Ok(())
            }
            _ => Err(ZxError::INVALID_ARGS),
        }
    }

    pub async fn sys_nanosleep(&self, deadline: Deadline) -> ZxResult {
        info!("nanosleep: deadline={:?}", deadline);
        if deadline.0 <= 0 {
            yield_now().await;
        } else {
            self.thread
                .blocking_run(
                    sleep_until(deadline.into()),
                    ThreadState::BlockedSleeping,
                    Duration::from_nanos(u64::max_value()),
                )
                .await
                .unwrap();
        }
        Ok(())
    }
}

#[repr(transparent)]
#[derive(Debug)]
pub struct Deadline(i64);

impl From<usize> for Deadline {
    fn from(x: usize) -> Self {
        Deadline(x as i64)
    }
}

impl From<Deadline> for Duration {
    fn from(deadline: Deadline) -> Self {
        Duration::from_nanos(deadline.0.max(0) as u64)
    }
}
