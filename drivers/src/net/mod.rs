//! LAN driver, only for Realtek currently.
#![allow(unused)]

use alloc::{sync::Arc, vec};
use lock::Mutex;
use smoltcp::socket::SocketSet;

pub mod e1000;
pub mod e1000e;
// pub mod ixgbe;
pub mod loopback;
pub use isomorphic_drivers::provider::Provider;
pub use loopback::{LoopbackDevice, LoopbackInterface};

use crate::scheme::{IrqScheme, Scheme};
use crate::DeviceResult;
use alloc::vec::Vec;

static MSI_IRQ_HOST: Mutex<Option<Arc<dyn IrqScheme>>> = Mutex::new(None);
static MSI_PENDING: Mutex<Vec<(usize, Arc<dyn Scheme>)>> = Mutex::new(Vec::new());
const MAX_MSI_PENDING: usize = 256;

fn enqueue_pending_msi(
    pending: &mut Vec<(usize, Arc<dyn Scheme>)>,
    vector: usize,
    dev: &Arc<dyn Scheme>,
) {
    if pending
        .iter()
        .any(|(v, d)| *v == vector && core::ptr::eq(Arc::as_ptr(d), Arc::as_ptr(dev)))
    {
        return;
    }
    if pending.len() >= MAX_MSI_PENDING {
        pending.remove(0);
    }
    pending.push((vector, dev.clone()));
}

pub fn pci_set_irq_host(irq: Arc<dyn IrqScheme>) {
    *MSI_IRQ_HOST.lock() = Some(irq);
}

pub fn pci_note_pending_msi(vector: usize, dev: Arc<dyn Scheme>) {
    enqueue_pending_msi(&mut MSI_PENDING.lock(), vector, &dev);
}

/// Register an ISR closure for MSI `vector` and unmask it, immediately (not
/// deferred like `pci_note_pending_msi`). Returns true on success. Used by the
/// NVIDIA console-GPU boot to bring the GPU's MSI delivery online for the
/// SEC2-resume window (the Linux-faithful interrupt path) rather than running
/// fully INTx-masked. Shared here because `net` owns the IRQ host handle.
pub fn msi_register_and_unmask(vector: usize, handler: crate::scheme::IrqHandler) -> bool {
    let host = MSI_IRQ_HOST.lock().clone();
    if let Some(host) = host {
        if host.register_handler(vector, handler).is_ok() {
            let _ = host.unmask(vector);
            return true;
        }
    }
    false
}

/// Mask + unregister an MSI `vector` previously brought online with
/// [`msi_register_and_unmask`].
pub fn msi_mask_and_unregister(vector: usize) {
    let host = MSI_IRQ_HOST.lock().clone();
    if let Some(host) = host {
        let _ = host.mask(vector);
        let _ = host.unregister(vector);
    }
}

/// Mask an MSI `vector` without unregistering (the storm self-limiter calls
/// this from inside the ISR).
pub fn msi_mask(vector: usize) {
    let host = MSI_IRQ_HOST.lock().clone();
    if let Some(host) = host {
        let _ = host.mask(vector);
    }
}

pub fn pci_finish_msi_registrations() -> DeviceResult {
    let host = MSI_IRQ_HOST.lock().clone();
    if let Some(host) = host {
        let mut q = MSI_PENDING.lock();
        for (v, d) in q.drain(..) {
            match host.register_device(v, d) {
                Ok(_) => {
                    crate::klog_info!("[net] IRQ vector {} registered for NIC", v);
                    let _ = host.unmask(v);
                }
                Err(e) => crate::klog_warn!("[net] failed to register IRQ vector {}: {:?}", v, e),
            }
        }
    }
    Ok(())
}

cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
mod realtek;
mod rtlx;

pub use rtlx::*;
    }
}

/*
/// External functions that drivers must use
pub trait Provider {
    /// Page size (usually 4K)
    const PAGE_SIZE: usize;

    /// Allocate consequent physical memory for DMA.
    /// Return (`virtual address`, `physical address`).
    /// The address is page aligned.
    fn alloc_dma(size: usize) -> (usize, usize);

    /// Deallocate DMA
    fn dealloc_dma(vaddr: usize, size: usize);
}
*/

pub struct ProviderImpl;

impl Provider for ProviderImpl {
    const PAGE_SIZE: usize = PAGE_SIZE;

    fn alloc_dma(size: usize) -> (usize, usize) {
        let paddr = unsafe { drivers_dma_alloc(size / PAGE_SIZE) };
        let vaddr = phys_to_virt(paddr);
        (vaddr, paddr)
    }

    fn dealloc_dma(vaddr: usize, size: usize) {
        let paddr = virt_to_phys(vaddr);
        unsafe { drivers_dma_dealloc(paddr, size / PAGE_SIZE) };
    }
}

pub fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
    unsafe { drivers_phys_to_virt(paddr) }
}

pub fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
    unsafe { drivers_virt_to_phys(vaddr) }
}

pub fn timer_now_as_micros() -> u64 {
    unsafe { drivers_timer_now_as_micros() }
}

pub fn intr_on() {
    unsafe { drivers_intr_on() }
}

pub fn intr_off() {
    unsafe { drivers_intr_off() }
}

pub fn intr_get() -> bool {
    unsafe { drivers_intr_get() }
}

extern "C" {
    fn drivers_dma_alloc(pages: usize) -> PhysAddr;
    fn drivers_dma_dealloc(paddr: PhysAddr, pages: usize) -> i32;
    fn drivers_phys_to_virt(paddr: PhysAddr) -> VirtAddr;
    fn drivers_virt_to_phys(vaddr: VirtAddr) -> PhysAddr;
    fn drivers_timer_now_as_micros() -> u64;
    fn drivers_intr_on();
    fn drivers_intr_off();
    fn drivers_intr_get() -> bool;
    fn drivers_wake_net_rx_waiters();
}

/// Wake all tasks waiting for TCP/UDP RX data.
/// Call this after `iface.poll()` processes incoming packets.
pub fn wake_net_rx_waiters() {
    unsafe { drivers_wake_net_rx_waiters() }
}

pub const PAGE_SIZE: usize = 4096;

type VirtAddr = usize;
type PhysAddr = usize;

lazy_static::lazy_static! {
    pub static ref SOCKETS: Arc<Mutex<SocketSet<'static>>> =
    Arc::new(Mutex::new(SocketSet::new(vec![])));

    static ref PACKET_CALLBACK: Mutex<Option<fn(&[u8])>> = Mutex::new(None);

    /// AF_PACKET taps queued during smoltcp `poll` (SOCKETS held) and flushed after.
    pub static ref DEFERRED_PACKETS: Mutex<alloc::collections::VecDeque<Vec<u8>>> = Mutex::new(alloc::collections::VecDeque::new());
}

const DEFERRED_PACKET_MAX: usize = 32;

/// Sets a callback for every received packet (raw).
pub fn set_packet_callback(callback: fn(&[u8])) {
    // Mutex::lock() uses push_off/pop_off which handles interrupt disabling.
    // Manual intr_off/on here bypasses noff accounting and causes
    // "RefCell already borrowed" panics under SMP.
    *PACKET_CALLBACK.lock() = Some(callback);
}

/// Dispatches a received packet to the registered callback.
pub fn net_dispatch_packet(data: &[u8]) {
    // Hot path for AF_PACKET / edhcpc — keep quiet (was warn! per packet).
    // Copy the fn pointer and drop the mutex before invoking — `push_packet` locks
    // more mutexes; holding PACKET_CALLBACK across the call nests push_off/pop_off
    // badly with manual intr_on/intr_off and panics with "RefCell already borrowed".
    let callback = *PACKET_CALLBACK.lock();
    if let Some(cb) = callback {
        cb(data);
    }
}

/// Queue a frame for AF_PACKET while smoltcp holds `SOCKETS` (see [`net_flush_deferred_packets`]).
pub fn net_defer_packet(data: &[u8]) {
    let mut q = DEFERRED_PACKETS.lock();
    if q.len() >= DEFERRED_PACKET_MAX {
        q.pop_front();
    }
    q.push_back(data.to_vec());
}

/// Flush frames queued by [`net_defer_packet`]; call only after releasing smoltcp locks.
pub fn net_flush_deferred_packets() {
    let batch: alloc::collections::VecDeque<Vec<u8>> = {
        let mut q = DEFERRED_PACKETS.lock();
        core::mem::take(&mut *q)
    };
    for pkt in batch {
        net_dispatch_packet(&pkt);
    }
}

// 注意！这个容易出现死锁
pub fn get_sockets() -> Arc<Mutex<SocketSet<'static>>> {
    SOCKETS.clone()
}
