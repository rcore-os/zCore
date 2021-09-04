//! Zircon syscall implementations

#![no_std]
#![deny(warnings, unsafe_code, unused_must_use, unreachable_patterns)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use {
    self::time::Deadline,
    alloc::sync::Arc,
    core::{
        convert::TryFrom,
        sync::atomic::{AtomicI32, Ordering},
    },
    futures::pin_mut,
    kernel_hal::user::*,
    zircon_object::object::*,
    zircon_object::task::{CurrentThread, ThreadFn},
};

mod channel;
mod consts;
mod cprng;
mod ddk;
mod debug;
mod debuglog;
mod exception;
mod fifo;
mod futex;
mod handle;
#[cfg(feature = "hypervisor")]
mod hypervisor;
mod object;
mod pci;
mod port;
mod resource;
mod signal;
mod socket;
mod stream;
mod system;
mod task;
mod time;
mod vmar;
mod vmo;

use consts::SyscallType as Sys;

pub struct Syscall<'a> {
    pub thread: &'a CurrentThread,
    pub thread_fn: ThreadFn,
}

impl Syscall<'_> {
    pub async fn syscall(&mut self, num: u32, args: [usize; 8]) -> isize {
        let thread_name = self.thread.name();
        let proc_name = self.thread.proc().name();
        let sys_type = match Sys::try_from(num) {
            Ok(t) => t,
            Err(_) => {
                error!("invalid syscall number: {}", num);
                return ZxError::INVALID_ARGS as _;
            }
        };
        debug!(
            "{}|{} {:?} => args={:x?}",
            proc_name, thread_name, sys_type, args
        );
        let [a0, a1, a2, a3, a4, a5, a6, a7] = args;
        let ret = match sys_type {
            Sys::HANDLE_CLOSE => self.sys_handle_close(a0 as _),
            Sys::HANDLE_CLOSE_MANY => self.sys_handle_close_many(a0.into(), a1 as _),
            Sys::HANDLE_DUPLICATE => self.sys_handle_duplicate(a0 as _, a1 as _, a2.into()),
            Sys::HANDLE_REPLACE => self.sys_handle_replace(a0 as _, a1 as _, a2.into()),
            Sys::OBJECT_GET_INFO => {
                self.sys_object_get_info(a0 as _, a1 as _, a2 as _, a3 as _, a4.into(), a5.into())
            }
            Sys::OBJECT_GET_PROPERTY => {
                self.sys_object_get_property(a0 as _, a1 as _, a2 as _, a3 as _)
            }
            Sys::OBJECT_SET_PROPERTY => {
                self.sys_object_set_property(a0 as _, a1 as _, a2 as _, a3 as _)
            }
            Sys::OBJECT_SIGNAL => self.sys_object_signal(a0 as _, a1 as _, a2 as _),
            Sys::OBJECT_SIGNAL_PEER => self.sys_object_signal_peer(a0 as _, a1 as _, a2 as _),
            Sys::OBJECT_WAIT_ONE => {
                self.sys_object_wait_one(a0 as _, a1 as _, a2.into(), a3.into())
                    .await
            }
            Sys::OBJECT_WAIT_MANY => {
                self.sys_object_wait_many(a0.into(), a1 as _, a2.into())
                    .await
            }
            Sys::OBJECT_WAIT_ASYNC => {
                self.sys_object_wait_async(a0 as _, a1 as _, a2 as _, a3 as _, a4 as _)
            }
            Sys::THREAD_CREATE => {
                self.sys_thread_create(a0 as _, a1.into(), a2 as _, a3 as _, a4.into())
            }
            Sys::THREAD_START => self.sys_thread_start(a0 as _, a1 as _, a2 as _, a3 as _, a4 as _),
            Sys::THREAD_WRITE_STATE => {
                self.sys_thread_write_state(a0 as _, a1 as _, a2.into(), a3 as _)
            }
            Sys::THREAD_READ_STATE => {
                self.sys_thread_read_state(a0 as _, a1 as _, a2.into(), a3 as _)
            }
            Sys::TASK_KILL => self.sys_task_kill(a0 as _),
            Sys::THREAD_EXIT => self.sys_thread_exit(),
            Sys::PROCESS_CREATE => {
                self.sys_process_create(a0 as _, a1.into(), a2 as _, a3 as _, a4.into(), a5.into())
            }
            Sys::PROCESS_START => {
                self.sys_process_start(a0 as _, a1 as _, a2 as _, a3 as _, a4 as _, a5 as _)
            }
            Sys::PROCESS_READ_MEMORY => {
                self.sys_process_read_memory(a0 as _, a1 as _, a2.into(), a3 as _, a4.into())
            }
            Sys::PROCESS_WRITE_MEMORY => {
                self.sys_process_write_memory(a0 as _, a1 as _, a2.into(), a3 as _, a4.into())
            }
            Sys::PROCESS_EXIT => self.sys_process_exit(a0 as _),
            Sys::JOB_CREATE => self.sys_job_create(a0 as _, a1 as _, a2.into()),
            Sys::JOB_SET_POLICY => self.sys_job_set_policy(a0 as _, a1 as _, a2 as _, a3, a4 as _),
            Sys::JOB_SET_CRITICAL => self.sys_job_set_critical(a0 as _, a1 as _, a2 as _),
            Sys::TASK_SUSPEND | Sys::TASK_SUSPEND_TOKEN => {
                self.sys_task_suspend_token(a0 as _, a1.into())
            }
            Sys::CHANNEL_CREATE => self.sys_channel_create(a0 as _, a1.into(), a2.into()),
            Sys::CHANNEL_READ => self.sys_channel_read(
                a0 as _,
                a1 as _,
                a2.into(),
                a3 as _,
                a4 as _,
                a5 as _,
                a6.into(),
                a7.into(),
                false,
            ),
            Sys::CHANNEL_READ_ETC => self.sys_channel_read(
                a0 as _,
                a1 as _,
                a2.into(),
                a3 as _,
                a4 as _,
                a5 as _,
                a6.into(),
                a7.into(),
                true,
            ),
            Sys::CHANNEL_WRITE => {
                self.sys_channel_write(a0 as _, a1 as _, a2.into(), a3 as _, a4.into(), a5 as _)
            }
            Sys::CHANNEL_WRITE_ETC => {
                self.sys_channel_write_etc(a0 as _, a1 as _, a2.into(), a3 as _, a4.into(), a5 as _)
            }
            Sys::CHANNEL_CALL_NORETRY => {
                self.sys_channel_call_noretry(
                    a0 as _,
                    a1 as _,
                    a2.into(),
                    a3.into(),
                    a4.into(),
                    a5.into(),
                )
                .await
            }
            Sys::CHANNEL_CALL_FINISH => {
                self.sys_channel_call_finish(a0.into(), a1.into(), a2.into(), a3.into())
            }
            Sys::SOCKET_CREATE => self.sys_socket_create(a0 as _, a1.into(), a2.into()),
            Sys::SOCKET_WRITE => {
                self.sys_socket_write(a0 as _, a1 as _, a2.into(), a3 as _, a4.into())
            }
            Sys::SOCKET_READ => {
                self.sys_socket_read(a0 as _, a1 as _, a2.into(), a3 as _, a4.into())
            }
            Sys::SOCKET_SHUTDOWN => self.sys_socket_shutdown(a0 as _, a1 as _),
            Sys::STREAM_CREATE => self.sys_stream_create(a0 as _, a1 as _, a2 as _, a3.into()),
            Sys::STREAM_WRITEV => {
                self.sys_stream_writev(a0 as _, a1 as _, a2.into(), a3 as _, a4.into())
            }
            Sys::STREAM_WRITEV_AT => {
                self.sys_stream_writev_at(a0 as _, a1 as _, a2 as _, a3.into(), a4 as _, a5.into())
            }
            Sys::STREAM_READV => {
                self.sys_stream_readv(a0 as _, a1 as _, a2.into(), a3 as _, a4.into())
            }
            Sys::STREAM_READV_AT => {
                self.sys_stream_readv_at(a0 as _, a1 as _, a2 as _, a3.into(), a4 as _, a5.into())
            }
            Sys::STREAM_SEEK => self.sys_stream_seek(a0 as _, a1 as _, a2 as _, a3.into()),
            Sys::FIFO_CREATE => {
                self.sys_fifo_create(a0 as _, a1 as _, a2 as _, a3.into(), a4.into())
            }
            Sys::FIFO_READ => self.sys_fifo_read(a0 as _, a1 as _, a2.into(), a3 as _, a4.into()),
            Sys::FIFO_WRITE => self.sys_fifo_write(a0 as _, a1 as _, a2.into(), a3 as _, a4.into()),
            Sys::EVENT_CREATE => self.sys_event_create(a0 as _, a1.into()),
            Sys::EVENTPAIR_CREATE => self.sys_eventpair_create(a0 as _, a1.into(), a2.into()),
            Sys::PORT_CREATE => self.sys_port_create(a0 as _, a1.into()),
            Sys::PORT_WAIT => self.sys_port_wait(a0 as _, a1.into(), a2.into()).await,
            Sys::PORT_QUEUE => self.sys_port_queue(a0 as _, a1.into()),
            Sys::PORT_CANCEL => {
                error!("Skip PORT_CANCEL");
                Ok(())
            }
            Sys::FUTEX_WAIT => {
                self.sys_futex_wait(a0.into(), a1 as _, a2 as _, a3.into())
                    .await
            }
            Sys::FUTEX_WAKE => self.sys_futex_wake(a0.into(), a1 as _),
            Sys::FUTEX_REQUEUE => {
                self.sys_futex_requeue(a0.into(), a1 as _, a2 as _, a3.into(), a4 as _, a5 as _)
            }
            Sys::FUTEX_WAKE_SINGLE_OWNER => self.sys_futex_wake_single_owner(a0.into()),
            Sys::VMO_CREATE => self.sys_vmo_create(a0 as _, a1 as _, a2.into()),
            Sys::VMO_READ => self.sys_vmo_read(a0 as _, a1.into(), a2 as _, a3 as _),
            Sys::VMO_WRITE => self.sys_vmo_write(a0 as _, a1.into(), a2 as _, a3 as _),
            Sys::VMO_GET_SIZE => self.sys_vmo_get_size(a0 as _, a1.into()),
            Sys::VMO_SET_SIZE => self.sys_vmo_set_size(a0 as _, a1 as _),
            Sys::VMO_OP_RANGE => {
                self.sys_vmo_op_range(a0 as _, a1 as _, a2 as _, a3 as _, a4.into(), a5 as _)
            }
            Sys::VMO_REPLACE_AS_EXECUTABLE => {
                self.sys_vmo_replace_as_executable(a0 as _, a1 as _, a2.into())
            }
            Sys::VMO_CREATE_CHILD => {
                self.sys_vmo_create_child(a0 as _, a1 as _, a2 as _, a3 as _, a4.into())
            }
            Sys::VMO_CREATE_PHYSICAL => {
                self.sys_vmo_create_physical(a0 as _, a1 as _, a2 as _, a3.into())
            }
            Sys::VMO_CREATE_CONTIGUOUS => {
                self.sys_vmo_create_contiguous(a0 as _, a1 as _, a2 as _, a3.into())
            }
            Sys::VMO_SET_CACHE_POLICY => self.sys_vmo_cache_policy(a0 as _, a1 as _),
            Sys::VMAR_MAP => self.sys_vmar_map(
                a0 as _,
                a1 as _,
                a2 as _,
                a3 as _,
                a4 as _,
                a5 as _,
                a6.into(),
            ),
            Sys::VMAR_UNMAP => self.sys_vmar_unmap(a0 as _, a1 as _, a2 as _),
            Sys::VMAR_ALLOCATE => {
                self.sys_vmar_allocate(a0 as _, a1 as _, a2 as _, a3 as _, a4.into(), a5.into())
            }
            Sys::VMAR_PROTECT => self.sys_vmar_protect(a0 as _, a1 as _, a2 as _, a3 as _),
            Sys::VMAR_DESTROY => self.sys_vmar_destroy(a0 as _),
            Sys::CPRNG_DRAW_ONCE => self.sys_cprng_draw_once(a0.into(), a1 as _),
            Sys::NANOSLEEP => self.sys_nanosleep(a0.into()).await,
            Sys::CLOCK_CREATE => self.sys_clock_create(a0 as _, a1.into(), a2.into()),
            Sys::CLOCK_GET => self.sys_clock_get(a0 as _, a1.into()),
            Sys::CLOCK_READ => self.sys_clock_read(a0 as _, a1.into()),
            Sys::CLOCK_ADJUST => self.sys_clock_adjust(a0 as _, a1 as _, a2 as _),
            Sys::CLOCK_UPDATE => self.sys_clock_update(a0 as _, a1 as _, a2.into()),
            Sys::TIMER_CREATE => self.sys_timer_create(a0 as _, a1 as _, a2.into()),
            Sys::DEBUG_WRITE => self.sys_debug_write(a0.into(), a1 as _),
            Sys::DEBUGLOG_CREATE => self.sys_debuglog_create(a0 as _, a1 as _, a2.into()),
            Sys::DEBUGLOG_WRITE => self.sys_debuglog_write(a0 as _, a1 as _, a2.into(), a3 as _),
            Sys::DEBUGLOG_READ => self.sys_debuglog_read(a0 as _, a1 as _, a2.into(), a3 as _),
            Sys::RESOURCE_CREATE => self.sys_resource_create(
                a0 as _,
                a1 as _,
                a2 as _,
                a3 as _,
                a4.into(),
                a5 as _,
                a6.into(),
            ),
            Sys::SYSTEM_GET_EVENT => self.sys_system_get_event(a0 as _, a1 as _, a2.into()),
            Sys::TIMER_SET => self.sys_timer_set(a0 as _, a1.into(), a2 as _),
            Sys::TIMER_CANCEL => self.sys_timer_cancel(a0 as _),
            Sys::DEBUG_READ => {
                self.sys_debug_read(a0 as _, a1.into(), a2 as _, a3.into())
                    .await
            }
            Sys::TASK_CREATE_EXCEPTION_CHANNEL => {
                self.sys_create_exception_channel(a0 as _, a1 as _, a2.into())
            }
            Sys::IOMMU_CREATE => {
                self.sys_iommu_create(a0 as _, a1 as _, a2.into(), a3 as _, a4.into())
            }
            Sys::BTI_CREATE => self.sys_bti_create(a0 as _, a1 as _, a2 as _, a3.into()),
            Sys::BTI_PIN => self.sys_bti_pin(
                a0 as _,
                a1 as _,
                a2 as _,
                a3 as _,
                a4 as _,
                a5.into(),
                a6 as _,
                a7.into(),
            ),
            Sys::PMT_UNPIN => self.sys_pmt_unpin(a0 as _),
            Sys::BTI_RELEASE_QUARANTINE => self.sys_bti_release_quarantine(a0 as _),
            Sys::VMAR_UNMAP_HANDLE_CLOSE_THREAD_EXIT => self
                .sys_vmar_unmap(a0 as _, a1 as _, a2 as _)
                .and_then(|_| {
                    let _ = self.sys_handle_close(a3 as _);
                    self.sys_thread_exit()
                }),
            Sys::FUTEX_WAKE_HANDLE_CLOSE_THREAD_EXIT => {
                // atomic_store_explicit(value_ptr, new_value, memory_order_release)
                UserInPtr::<AtomicI32>::from(a0)
                    .as_ref()
                    .unwrap()
                    .store(a2 as i32, Ordering::Release);
                let _ = self.sys_futex_wake(a0.into(), a1 as _);
                let _ = self.sys_handle_close(a3 as _);
                self.sys_thread_exit()
            }
            Sys::OBJECT_GET_CHILD => {
                self.sys_object_get_child(a0 as _, a1 as _, a2 as _, a3.into())
            }
            Sys::PC_FIRMWARE_TABLES => self.sys_pc_firmware_tables(a0 as _, a1.into(), a2.into()),
            Sys::PCI_ADD_SUBTRACT_IO_RANGE => {
                self.sys_pci_add_subtract_io_range(a0 as _, a1 != 0, a2 as _, a3 as _, a4 != 0)
            }
            Sys::PCI_CFG_PIO_RW => self.sys_pci_cfg_pio_rw(
                a0 as _,
                a1 as _,
                a2 as _,
                a3 as _,
                a4 as _,
                a5.into(),
                a6 as _,
                a7 != 0,
            ),
            Sys::PCI_INIT => self.sys_pci_init(a0 as _, a1 as _, a2 as _),
            Sys::PCI_GET_NTH_DEVICE => {
                self.sys_pci_get_nth_device(a0 as _, a1 as _, a2.into(), a3.into())
            }
            Sys::PCI_MAP_INTERRUPT => self.sys_pci_map_interrupt(a0 as _, a1 as _, a2.into()),
            Sys::PCI_GET_BAR => self.sys_pci_get_bar(a0 as _, a1 as _, a2.into(), a3.into()),
            Sys::PCI_ENABLE_BUS_MASTER => self.sys_pci_enable_bus_master(a0 as _, a1 != 0),
            Sys::PCI_QUERY_IRQ_MODE => self.sys_pci_query_irq_mode(a0 as _, a1 as _, a2.into()),
            Sys::PCI_SET_IRQ_MODE => self.sys_pci_set_irq_mode(a0 as _, a1 as _, a2 as _),
            Sys::PCI_CONFIG_READ => self.sys_pci_config_read(a0 as _, a1 as _, a2 as _, a3.into()),
            Sys::PCI_CONFIG_WRITE => self.sys_pci_config_write(a0 as _, a1 as _, a2 as _, a3 as _),
            Sys::INTERRUPT_CREATE => {
                self.sys_interrupt_create(a0 as _, a1 as _, a2 as _, a3.into())
            }
            Sys::INTERRUPT_BIND => self.sys_interrupt_bind(a0 as _, a1 as _, a2 as _, a3 as _),
            Sys::INTERRUPT_TRIGGER => self.sys_interrupt_trigger(a0 as _, a1 as _, a2 as _),
            Sys::INTERRUPT_ACK => self.sys_interrupt_ack(a0 as _),
            Sys::INTERRUPT_DESTROY => self.sys_interrupt_destroy(a0 as _),
            Sys::INTERRUPT_WAIT => self.sys_interrupt_wait(a0 as _, a1.into()).await,
            Sys::EXCEPTION_GET_THREAD => self.sys_exception_get_thread(a0 as _, a1.into()),
            Sys::EXCEPTION_GET_PROCESS => self.sys_exception_get_process(a0 as _, a1.into()),
            Sys::IOPORTS_REQUEST => {
                warn!("ioports.request: skip");
                Ok(())
            }
            #[cfg(feature = "hypervisor")]
            Sys::GUEST_CREATE => self.sys_guest_create(a0 as _, a1 as _, a2.into(), a3.into()),
            #[cfg(feature = "hypervisor")]
            Sys::GUEST_SET_TRAP => {
                self.sys_guest_set_trap(a0 as _, a1 as _, a2 as _, a3 as _, a4 as _, a5 as _)
            }
            #[cfg(feature = "hypervisor")]
            Sys::VCPU_CREATE => self.sys_vcpu_create(a0 as _, a1 as _, a2 as _, a3.into()),
            #[cfg(feature = "hypervisor")]
            Sys::VCPU_RESUME => self.sys_vcpu_resume(a0 as _, a1.into()),
            #[cfg(feature = "hypervisor")]
            Sys::VCPU_INTERRUPT => self.sys_vcpu_interrupt(a0 as _, a1 as _),
            #[cfg(feature = "hypervisor")]
            Sys::VCPU_READ_STATE => self.sys_vcpu_read_state(a0 as _, a1 as _, a2.into(), a3 as _),
            #[cfg(feature = "hypervisor")]
            Sys::VCPU_WRITE_STATE => self.sys_vcpu_write_state(a0 as _, a1 as _, a2, a3 as _),
            _ => {
                error!("syscall unimplemented: {:?}", sys_type);
                Err(ZxError::NOT_SUPPORTED)
            }
        };
        info!("{}|{} {:?} <= {:?}", proc_name, thread_name, sys_type, ret);
        match ret {
            Ok(_) => 0,
            Err(err) => err as isize,
        }
    }
}
