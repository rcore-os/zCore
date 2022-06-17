//! LAN driver, only for Realtek currently.

pub mod e1000;
pub mod loopback;
use alloc::sync::Arc;
use alloc::vec;
pub use isomorphic_drivers::provider::Provider;
use lock::Mutex;
pub use loopback::LoopbackInterface;
use smoltcp::socket::SocketSet;

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

extern "C" {
    fn drivers_dma_alloc(pages: usize) -> PhysAddr;
    fn drivers_dma_dealloc(paddr: PhysAddr, pages: usize) -> i32;
    fn drivers_phys_to_virt(paddr: PhysAddr) -> VirtAddr;
    fn drivers_virt_to_phys(vaddr: VirtAddr) -> PhysAddr;
    fn drivers_timer_now_as_micros() -> u64;
}

pub const PAGE_SIZE: usize = 4096;

type VirtAddr = usize;
type PhysAddr = usize;

lazy_static::lazy_static! {
    pub static ref SOCKETS: Arc<Mutex<SocketSet<'static>>> =
    Arc::new(Mutex::new(SocketSet::new(vec![])));
}

pub fn get_sockets() -> Arc<Mutex<SocketSet<'static>>> {
    SOCKETS.clone()
}
