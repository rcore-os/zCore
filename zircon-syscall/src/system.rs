#![allow(dead_code)]
use {
    super::*,
    zircon_object::{signal::Event, task::Job},
    alloc::boxed::Box,
};

impl Syscall<'_> {
    pub fn sys_system_get_event(
        &self,
        root_job: HandleValue,
        kind: u32,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!("root_job={:#x}, kind={:#x}, out_ptr={:#x?}", root_job, kind, out);
        match kind {
            EVENT_OUT_OF_MEMORY => {
                let proc = self.thread.proc();
                proc
                    .get_object_with_rights::<Job>(root_job, Rights::MANAGE_PROCESS)?
                    .check_root_job()?;
                let event = Event::new();
                event.add_signal_callback(Box::new(|_| {
                    panic!("Out Of Memory!");
                }));
                let event_handle = proc.add_handle(Handle::new(event, Rights::DEFAULT_EVENT));
                out.write(event_handle)?;
                Ok(())
            },
            _ => unimplemented!()
        }
    }
}

const EVENT_OUT_OF_MEMORY: u32 = 1;
const EVENT_MEMORY_PRESSURE_CRITICAL: u32 = 2;
const EVENT_MEMORY_PRESSURE_WARNING: u32 = 3;
const EVENT_MEMORY_PRESSURE_NORMAL: u32 = 4;
