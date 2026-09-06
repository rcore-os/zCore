use super::*;
use zircon_object::{signal::Event, task::Job};

impl Syscall<'_> {
    /// Retrieve a handle to a system event.
    ///
    /// `root_job: HandleValue`, must be a handle to the root job of the system.
    /// `kind: u32`, must be one of the following:
    /// ```rust
    ///     const EVENT_OUT_OF_MEMORY: u32 = 1;
    ///     const EVENT_MEMORY_PRESSURE_CRITICAL: u32 = 2;
    ///     const EVENT_MEMORY_PRESSURE_WARNING: u32 = 3;
    ///     const EVENT_MEMORY_PRESSURE_NORMAL: u32 = 4;
    /// ```
    pub fn sys_system_get_event(
        &self,
        root_job: HandleValue,
        kind: u32,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "system.get_event: root_job={:#x}, kind={:#x}, out_ptr={:#x?}",
            root_job, kind, out
        );
        if !matches!(
            kind,
            EVENT_OUT_OF_MEMORY
                | EVENT_MEMORY_PRESSURE_CRITICAL
                | EVENT_MEMORY_PRESSURE_WARNING
                | EVENT_MEMORY_PRESSURE_NORMAL
                | EVENT_IMMINENT_OUT_OF_MEMORY
        ) {
            return Err(ZxError::INVALID_ARGS);
        }

        let proc = self.thread.proc();
        proc.get_object_with_rights::<Job>(root_job, Rights::MANAGE_PROCESS)?
            .check_root_job()?;
        // Expose the normal-pressure event as the single initially signaled
        // event. Memory-pressure transitions are not modeled yet.
        let event = Event::new();
        if kind == EVENT_MEMORY_PRESSURE_NORMAL {
            event.signal_set(Signal::SIGNALED);
        }
        let event_handle =
            proc.add_handle(Handle::new(event, Rights::DEFAULT_SYSTEM_EVENT_LOW_MEMORY));
        out.write(event_handle)?;
        Ok(())
    }
}

const EVENT_OUT_OF_MEMORY: u32 = 1;
const EVENT_MEMORY_PRESSURE_CRITICAL: u32 = 2;
const EVENT_MEMORY_PRESSURE_WARNING: u32 = 3;
const EVENT_MEMORY_PRESSURE_NORMAL: u32 = 4;
const EVENT_IMMINENT_OUT_OF_MEMORY: u32 = 5;
