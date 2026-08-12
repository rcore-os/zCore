//! xHCI + USB HID: enumeración en puertos raíz, HID boot (teclado / ratón / tablet QEMU),
//! MSI + `poll()` por timer, handoff USB legacy, anillos TRB alineados a la especificación.
//!
//! **Alcance:** un controlador xHCI (registro global `POLL_INSTANCE`), sin hubs USB.
//! **No cubierto:** descriptores HID no boot, varios interfaces HID compuestos, USB3
//! recovery avanzado.

use alloc::sync::Arc;
use alloc::vec::Vec;
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{_mm_clflush, _mm_lfence, _mm_mfence};
use core::hint::spin_loop;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, AtomicBool, Ordering};

use lock::Mutex;
use pci::PCIDevice;

use crate::builder::IoMapper;
use crate::bus::drivers_timer_now_as_micros;
use crate::bus::pci_drivers::PciDriver;
use crate::bus::{phys_to_virt, PAGE_SIZE};
use crate::input::input_event_codes::{ev::*, key::*, rel::*, syn::*};
use crate::prelude::{CapabilityType, InputCapability, InputEvent, InputEventType};
use crate::scheme::{impl_event_scheme, InputScheme, IrqScheme, Scheme};
use crate::utils::EventListener;
use crate::{Device, DeviceError, DeviceResult};
use pci::BAR;

fn timer_now_us() -> u64 {
    unsafe { drivers_timer_now_as_micros() }
}

#[inline(always)]
fn xhci_wait_spin_limit(timeout_us: u64) -> u64 {
    // Cap spin iterations so a stuck TSC timer cannot block boot for minutes.
    // (Previously timeout_us * 50_000 allowed billions of iterations at 80%.)
    500_000_000u64.max(timeout_us.saturating_mul(500))
}

#[inline(always)]
fn xhci_wait_expired(start: u64, timeout_us: u64, spins: u64) -> bool {
    timer_now_us().wrapping_sub(start) >= timeout_us || spins >= xhci_wait_spin_limit(timeout_us)
}

#[inline(always)]
fn xhci_spin_delay_us(delay_us: u64) {
    let start = timer_now_us();
    let mut spins = 0u64;
    while !xhci_wait_expired(start, delay_us, spins) {
        spins = spins.saturating_add(1);
        spin_loop();
    }
}

// PORTSC bits RW1C (port change). Hay que mantenerlos a 0 salvo cuando queramos limpiarlos.
const PORTSC_CHANGE_BITS: u32 =
    (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22);

// PORTSC bits que son RW1C pero NO son "change" flags — escribir 1 los borra.
// PED (bit 1) es RW1C: escribir 1 deshabilita el puerto. Siempre hay que enmascararlo
// cuando modificamos PORTSC para no tirar accidentalmente la habilitación del puerto.
const PORTSC_RW1C_AND_RO_MASK: u32 = PORTSC_CHANGE_BITS | (1 << 1); // incluye PED
const XHCI_MAX_XECP_TRAVERSAL: usize = 256;
const XHCI_WAIT_SPIN_FACTOR: u64 = 50_000;

// ——— USB legacy (EHCI/OHCI/UHCI) ———
//
// El usuario pidió unificar: mantenemos el cableado aquí (aunque el nombre del
// fichero sea `xhci_hid.rs`) para evitar proliferación de módulos.

#[cfg(feature = "legacy-usb-hid")]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum LegacyUsbKind {
    Uhci,
    Ohci,
    Ehci,
}

#[cfg(feature = "legacy-usb-hid")]
pub struct LegacyUsbHid {
    #[allow(dead_code)]
    listener: EventListener<InputEvent>,
    #[allow(dead_code)]
    kind: LegacyUsbKind,
}

#[cfg(feature = "legacy-usb-hid")]
impl LegacyUsbHid {
    pub fn probe(
        kind: LegacyUsbKind,
        _dev: &PCIDevice,
        _mmio_vaddr: usize,
        _bar_size: usize,
        _msi_vector: usize,
    ) -> DeviceResult<Arc<Self>> {
        let _ = kind;
        Err(DeviceError::NotSupported)
    }
}

#[cfg(feature = "legacy-usb-hid")]
impl_event_scheme!(LegacyUsbHid, InputEvent);

#[cfg(feature = "legacy-usb-hid")]
impl Scheme for LegacyUsbHid {
    fn name(&self) -> &str {
        match self.kind {
            LegacyUsbKind::Uhci => "uhci-usb-hid",
            LegacyUsbKind::Ohci => "ohci-usb-hid",
            LegacyUsbKind::Ehci => "ehci-usb-hid",
        }
    }
}

#[cfg(feature = "legacy-usb-hid")]
impl InputScheme for LegacyUsbHid {
    fn capability(&self, _cap_type: CapabilityType) -> InputCapability {
        InputCapability::empty()
    }
}

// ——— MSI diferido ———

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

/// Set once the controller is observed HCHalted. A halted xHCI does not
/// auto-recover here (HCHalted only clears via an HCRST reset, which this
/// driver never issues), so once it trips, `poll()` short-circuits to a single
/// atomic load instead of taking two locks and doing MMIO on every timer tick
/// and every I/O-wait loop iteration — futile work that was adding latency to
/// e.g. stdin reads and slowing down I/O-heavy programs. If controller reset /
/// recovery is ever added, it must clear this flag.
static XHCI_HALTED: AtomicBool = AtomicBool::new(false);

pub fn pci_finish_msi_registrations() -> DeviceResult<()> {
    let host = MSI_IRQ_HOST.lock().clone().ok_or(DeviceError::NotReady)?;
    let mut q = MSI_PENDING.lock();
    for (v, d) in q.drain(..) {
        host.register_device(v, d)?;
        host.unmask(v)?;
    }
    Ok(())
}

unsafe fn dma_alloc_pages(pages: usize) -> DeviceResult<(usize, usize)> {
    let p = crate::bus::drivers_dma_alloc(pages);
    if p == 0 {
        return Err(DeviceError::DmaError);
    }
    Ok((phys_to_virt(p), p))
}

struct DmaBuf {
    virt: usize,
    phys: usize,
    len: usize,
}

impl DmaBuf {
    fn new(len: usize, align: usize) -> DeviceResult<Self> {
        let pages = (len + PAGE_SIZE - 1) / PAGE_SIZE;
        let ap = (align + PAGE_SIZE - 1) / PAGE_SIZE;
        let pages = pages.max(ap);
        let (virt, phys) = unsafe { dma_alloc_pages(pages)? };
        unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, pages * PAGE_SIZE);
        }
        Ok(Self {
            virt,
            phys,
            len: pages * PAGE_SIZE,
        })
    }

    fn write_u32(&self, off: usize, v: u32) {
        unsafe {
            ((self.virt + off) as *mut u32).write_volatile(v);
        }
    }

    fn read_u32(&self, off: usize) -> u32 {
        unsafe { ((self.virt + off) as *const u32).read_volatile() }
    }

    fn read_u64(&self, off: usize) -> u64 {
        let lo = self.read_u32(off) as u64;
        let hi = self.read_u32(off + 4) as u64;
        lo | (hi << 32)
    }

    fn write_u64(&self, off: usize, v: u64) {
        self.write_u32(off, v as u32);
        self.write_u32(off + 4, (v >> 32) as u32);
    }

    fn read_into(&self, off: usize, dst: &mut [u8]) {
        let n = dst.len().min(self.len.saturating_sub(off));
        if n == 0 {
            return;
        }
        unsafe {
            core::ptr::copy_nonoverlapping((self.virt + off) as *const u8, dst.as_mut_ptr(), n);
        }
    }

    fn sub_phys(&self, off: usize) -> u64 {
        (self.phys + off) as u64
    }

    fn flush(&self, off: usize, len: usize) {
        #[cfg(target_arch = "x86_64")]
        {
            let mut addr = self.virt + off;
            let end = addr + len;
            while addr < end {
                unsafe {
                    _mm_clflush(addr as *const u8);
                }
                addr += 64;
            }
            unsafe {
                _mm_mfence();
            }
        }
        let _ = (off, len);
    }
}

pub struct XhciMmio {
    cap_base: usize,
    cap_len: u64,
    op_base: usize,
    rt_base: usize,
    db_base: usize,
    bar_size: usize,
}

impl XhciMmio {
    pub fn from_virt(cap_base: usize, bar_size: usize) -> DeviceResult<Self> {
        if cap_base == 0 {
            return Err(DeviceError::InvalidParam);
        }
        let cap = cap_base;
        let caplength = (unsafe { read_volatile(cap as *const u32) } & 0xFF) as u64;
        let rtsoff = (unsafe { read_volatile((cap + 0x18) as *const u32) } & 0xFFFF_FFFC) as u64;
        let dboff = (unsafe { read_volatile((cap + 0x14) as *const u32) } & 0xFFFF_FFFC) as u64;
        if caplength as usize > bar_size || rtsoff as usize > bar_size || dboff as usize > bar_size
        {
            return Err(DeviceError::InvalidParam);
        }
        Ok(Self {
            cap_base: cap,
            cap_len: caplength,
            op_base: cap + caplength as usize,
            rt_base: cap + rtsoff as usize,
            db_base: cap + dboff as usize,
            bar_size,
        })
    }

    fn read_cap(&self, o: usize) -> u32 {
        fence(Ordering::Acquire);
        unsafe { read_volatile((self.cap_base + o) as *const u32) }
    }

    fn write_cap(&self, o: usize, v: u32) {
        fence(Ordering::Release);
        unsafe {
            write_volatile((self.cap_base + o) as *mut u32, v);
        }
        fence(Ordering::Release);
    }

    /// xHCI xECP: USB Legacy Support Capability — ceder control a la OS y desactivar SMI (metal).
    fn perform_bios_handoff(&self) {
        let hcc1 = self.read_cap(0x10);
        let xecp = (hcc1 >> 16) as usize;
        if xecp == 0 {
            return;
        }
        let mut cap_ptr = xecp << 2;
        let mut cap_steps = 0usize;
        while cap_ptr != 0 && cap_ptr < self.bar_size {
            if cap_steps >= XHCI_MAX_XECP_TRAVERSAL {
                warn!("[xhci] xECP chain demasiado larga/cíclica, abortando handoff");
                break;
            }
            cap_steps = cap_steps.saturating_add(1);
            let cap_val = self.read_cap(cap_ptr);
            let cap_id = (cap_val & 0xff) as u8;
            if cap_id == 1 {
                // USB Legacy Support
                let mut legsup = self.read_cap(cap_ptr);
                if (legsup & (1 << 16)) != 0 {
                    info!("[xhci] BIOS posee el controlador, solicitando handoff...");
                    legsup |= 1 << 24; // OS Owned Semaphore
                    self.write_cap(cap_ptr, legsup);

                    let start = timer_now_us();
                    let mut spins = 0u64;
                    let max_spins = xhci_wait_spin_limit(500_000);
                    while (self.read_cap(cap_ptr) & (1 << 16)) != 0
                        && (timer_now_us() - start) < 500_000
                        && spins < max_spins
                    {
                        spins = spins.saturating_add(1);
                        spin_loop();
                    }
                    if spins >= max_spins {
                        warn!("[xhci] handoff alcanzó guard de spins (timer estancado?)");
                    }

                    if (self.read_cap(cap_ptr) & (1 << 16)) != 0 {
                        warn!("[xhci] handoff fallido por timeout, forzando control");
                    } else {
                        info!("[xhci] handoff completado con éxito");
                    }
                }

                // Desactivar SMIs y limpiar estados pendientes (USBLEGCTLSTS = offset 4)
                // Escribir 0xFFFF0000 para limpiar bits RW1C y desactivar enable bits.
                if cap_ptr + 4 <= self.bar_size {
                    self.write_cap(cap_ptr + 4, 0xffff_0000);
                }
                return;
            }
            let next = ((cap_val >> 8) & 0xff) as usize;
            if next == 0 {
                break;
            }
            cap_ptr = cap_ptr.saturating_add(next << 2);
        }
    }

    fn ack_host_interrupt(&self) {
        let usbsts = self.read_op(0x04);
        if (usbsts & 0x08) != 0 {
            self.write_op(0x04, 0x08);
        }
        let iman = self.read_rt(0x20);
        self.write_rt(0x20, (iman & 0x02) | 0x01);
    }

    fn read_op(&self, o: usize) -> u32 {
        fence(Ordering::Acquire);
        let v = unsafe { read_volatile((self.op_base + o) as *const u32) };
        fence(Ordering::Acquire);
        v
    }

    fn write_op(&self, o: usize, v: u32) {
        fence(Ordering::Release);
        unsafe { write_volatile((self.op_base + o) as *mut u32, v) }
        fence(Ordering::Release);
    }

    fn write_op64(&self, o: usize, v: u64) {
        fence(Ordering::Release);
        unsafe { write_volatile((self.op_base + o) as *mut u64, v) }
        fence(Ordering::Release);
    }

    fn write_rt(&self, o: usize, v: u32) {
        fence(Ordering::Release);
        unsafe { write_volatile((self.rt_base + o) as *mut u32, v) }
        fence(Ordering::Release);
    }

    fn write_rt64(&self, o: usize, v: u64) {
        fence(Ordering::Release);
        unsafe { write_volatile((self.rt_base + o) as *mut u64, v) }
        fence(Ordering::Release);
    }

    fn read_rt(&self, o: usize) -> u32 {
        fence(Ordering::Acquire);
        let v = unsafe { read_volatile((self.rt_base + o) as *const u32) };
        fence(Ordering::Acquire);
        v
    }

    pub fn ring_db(&self, slot: u8, doorbell: u8) {
        fence(Ordering::Release);
        unsafe {
            write_volatile(
                (self.db_base + (slot as usize) * 4) as *mut u32,
                doorbell as u32,
            );
        }
        fence(Ordering::Release);
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Trb {
    p: u64,
    status: u32,
    ctrl: u32,
}

const TRB_LINK: u32 = 6 << 10;
const TRB_EVT_CMD_COMP: u32 = 33 << 10;
const TRB_EVT_TRANSFER: u32 = 32 << 10;
const TRB_EVT_PORT_STATUS: u32 = 34 << 10;
const TRB_CC_SUCCESS: u32 = 1;
const TRB_CC_SHORT: u32 = 13;

fn trb_link(phys: u64, cycle: bool) -> Trb {
    let mut c = TRB_LINK | (1 << 1);
    if cycle {
        c |= 1;
    }
    Trb {
        p: phys,
        status: 0,
        ctrl: c,
    }
}

fn trb_enable_slot() -> Trb {
    Trb {
        p: 0,
        status: 0,
        ctrl: (9 << 10) | 1,
    }
}

fn trb_disable_slot(slot: u8) -> Trb {
    Trb {
        p: 0,
        status: 0,
        ctrl: (10 << 10) | ((slot as u32) << 24) | 1,
    }
}

fn trb_address_device(input_ctx: u64, slot: u8) -> Trb {
    Trb {
        p: input_ctx,
        status: 0,
        ctrl: (11u32 << 10) | ((slot as u32) << 24),
    }
}

fn trb_configure_endpoint(input_ctx: u64, slot: u8) -> Trb {
    Trb {
        p: input_ctx,
        status: 0,
        ctrl: (12u32 << 10) | ((slot as u32) << 24),
    }
}

fn trb_reset_endpoint(slot: u8, dci: u8) -> Trb {
    Trb {
        p: 0,
        status: 0,
        ctrl: (14u32 << 10) | ((dci as u32) << 16) | ((slot as u32) << 24),
    }
}

fn trb_set_tr_dequeue_pointer(new_deq_phys: u64, dcs: bool, slot: u8, dci: u8) -> Trb {
    Trb {
        p: (new_deq_phys & !0xf) | (dcs as u64),
        status: 0,
        ctrl: (16u32 << 10) | ((dci as u32) << 16) | ((slot as u32) << 24),
    }
}

fn trb_setup(bmrt: u8, breq: u8, wvalue: u16, windex: u16, wlen: u16, trt: u8) -> Trb {
    let param = (bmrt as u64)
        | ((breq as u64) << 8)
        | ((wvalue as u64) << 16)
        | ((windex as u64) << 32)
        | ((wlen as u64) << 48);
    Trb {
        p: param,
        status: 8,
        ctrl: (2u32 << 10) | (1u32 << 6) | ((trt as u32) << 16),
    }
}

fn trb_data(buf: u64, len: u32, is_in: bool) -> Trb {
    let mut c = 3u32 << 10;
    if is_in {
        c |= 1 << 16;
    }
    Trb {
        p: buf,
        status: len & 0x1_ffff,
        ctrl: c,
    }
}

fn trb_status(is_in: bool, ioc: bool) -> Trb {
    let mut c = 4u32 << 10;
    if is_in {
        c |= 1 << 16;
    }
    if ioc {
        c |= 1 << 5;
    }
    Trb {
        p: 0,
        status: 0,
        ctrl: c,
    }
}

fn trb_normal(buf: u64, len: u16, ioc: bool) -> Trb {
    let mut c = 1u32 << 10;
    if ioc {
        c |= 1 << 5;
    }
    Trb {
        p: buf,
        status: (len as u32) & 0x1_ffff,
        ctrl: c,
    }
}

struct CmdRing {
    buf: DmaBuf,
    cap: usize,
    enq: usize,
    cycle: bool,
}

impl CmdRing {
    /// Offset del dword de control del TRB LINK (último TRB del segmento; índice = `cap` con `cap = n - 1`).
    #[inline]
    fn link_ctrl_off(&self) -> usize {
        self.cap * 16 + 12
    }

    fn sync_link_cycle_bit(&self) {
        let link_off = self.link_ctrl_off();
        // Preservar todos los bits excepto el bit 0 (Cycle).
        // El bit 1 (Toggle Cycle) debe permanecer en 1 para que el hardware invierta su ciclo.
        let mut c = self.buf.read_u32(link_off) & !1u32;
        if self.cycle {
            c |= 1;
        }
        fence(Ordering::Release);
        self.buf.write_u32(link_off, c);
        // Asegurar que el controlador vea el cambio del bit de ciclo en el TRB LINK.
        self.buf.flush(link_off, 4);
    }

    fn new(n: usize) -> DeviceResult<Self> {
        let buf = DmaBuf::new(n * 16, 64)?;
        let link = trb_link(buf.phys as u64, true);
        let off = (n - 1) * 16;
        buf.write_u64(off, link.p);
        buf.write_u32(off + 8, link.status);
        fence(Ordering::Release);
        buf.write_u32(off + 12, link.ctrl);
        buf.flush(off, 16);
        Ok(Self {
            buf,
            cap: n - 1,
            enq: 0,
            cycle: true,
        })
    }

    fn push(&mut self, mut t: Trb) -> DeviceResult<u64> {
        let phys = (self.buf.phys + self.enq * 16) as u64;
        let cycle_bit = if self.cycle { 1u32 } else { 0 };
        t.ctrl = (t.ctrl & !1) | cycle_bit;
        let off = self.enq * 16;
        self.buf.write_u64(off, t.p);
        self.buf.write_u32(off + 8, t.status);
        fence(Ordering::Release);
        self.buf.write_u32(off + 12, t.ctrl);
        self.buf.flush(off, 16);
        self.enq += 1;
        if self.enq >= self.cap {
            self.enq = 0;
            self.sync_link_cycle_bit();
            self.cycle = !self.cycle;
        } else if self.cap >= 4 && self.enq == self.cap / 2 {
            self.sync_link_cycle_bit();
        }
        Ok(phys)
    }

    fn crcr(&self) -> u64 {
        self.buf.phys as u64
    }
}

/// Anillo de transferencia (EP0 / interrupción) con seguimiento de dequeue software.
struct XferRing {
    buf: DmaBuf,
    cap: usize,
    enq: usize,
    xfer_deq: usize,
    cycle: bool,
}

impl XferRing {
    #[inline]
    fn link_ctrl_off(&self) -> usize {
        self.cap * 16 + 12
    }

    fn sync_link_cycle_bit(&self) {
        let link_off = self.link_ctrl_off();
        // Preservar todos los bits excepto el bit 0 (Cycle).
        let mut c = self.buf.read_u32(link_off) & !1u32;
        if self.cycle {
            c |= 1;
        }
        fence(Ordering::Release);
        self.buf.write_u32(link_off, c);
        // Asegurar que el controlador vea el cambio del bit de ciclo en el TRB LINK.
        self.buf.flush(link_off, 4);
    }

    fn new(n: usize) -> DeviceResult<Self> {
        let buf = DmaBuf::new(n * 16, 64)?;
        let link = trb_link(buf.phys as u64, true);
        let off = (n - 1) * 16;
        buf.write_u64(off, link.p);
        buf.write_u32(off + 8, link.status);
        fence(Ordering::Release);
        buf.write_u32(off + 12, link.ctrl);
        buf.flush(off, 16);
        Ok(Self {
            buf,
            cap: n - 1,
            enq: 0,
            xfer_deq: 0,
            cycle: true,
        })
    }

    fn ring_phys(&self) -> u64 {
        self.buf.phys as u64 | 1
    }

    fn is_full(&self) -> bool {
        (self.enq + 1) % self.cap == self.xfer_deq
    }

    fn push(&mut self, mut t: Trb) -> DeviceResult<u64> {
        if self.is_full() {
            return Err(DeviceError::NoResources);
        }
        let phys = (self.buf.phys + self.enq * 16) as u64;
        let cycle_bit = if self.cycle { 1u32 } else { 0 };
        t.ctrl = (t.ctrl & !1) | cycle_bit;
        let off = self.enq * 16;
        self.buf.write_u64(off, t.p);
        self.buf.write_u32(off + 8, t.status);
        fence(Ordering::Release);
        self.buf.write_u32(off + 12, t.ctrl);
        self.buf.flush(off, 16);
        self.enq += 1;
        if self.enq >= self.cap {
            self.enq = 0;
            self.sync_link_cycle_bit();
            self.cycle = !self.cycle;
        } else if self.cap >= 4 && self.enq == self.cap / 2 {
            self.sync_link_cycle_bit();
        }
        Ok(phys)
    }

    fn advance_dequeue(&mut self, n: usize) {
        for _ in 0..n {
            self.xfer_deq = (self.xfer_deq + 1) % self.cap;
        }
    }

    fn deq_phys(&self) -> u64 {
        (self.buf.phys + self.xfer_deq * 16) as u64
    }

    fn deq_cycle(&self) -> bool {
        if self.xfer_deq == self.enq {
            self.cycle
        } else {
            let off = self.xfer_deq * 16;
            let ctrl = self.buf.read_u32(off + 12);
            (ctrl & 1) != 0
        }
    }
}

struct EventRing {
    seg: DmaBuf,
    erst: DmaBuf,
    deq: usize,
    cycle: bool,
    ntrb: usize,
}

impl EventRing {
    fn new(n: usize) -> DeviceResult<Self> {
        let seg = DmaBuf::new(n * 16, 64)?;
        // Flush the event ring segment to physical memory so that:
        // 1. The xHC reads zeros (not stale garbage) for unwritten slots.
        // 2. Cache lines are clean so peek()'s clflush does not write dirty zeros
        //    back over DMA-written events on non-cache-coherent or partially-coherent
        //    environments (e.g. VMs with PCIe passthrough or IOMMU bypass).
        seg.flush(0, seg.len);
        let erst = DmaBuf::new(16, 64)?;
        erst.write_u64(0, seg.phys as u64);
        erst.write_u32(8, n as u32);
        erst.write_u32(12, 0);
        // Flush the ERST so the xHC can read it via DMA before the first event is posted.
        erst.flush(0, erst.len);
        Ok(Self {
            seg,
            erst,
            deq: 0,
            cycle: true,
            ntrb: n,
        })
    }

    fn peek(&self) -> Option<Trb> {
        let off = self.deq * 16;
        // El controlador xHCI escribe eventos en RAM via DMA.
        // En x86_64, aunque el hardware hace cache snooping en la mayoría de sistemas,
        // algunos entornos (IOMMU no coherente, VMs con passthrough parcial, etc.)
        // pueden dejar la línea de caché marcada como válida con los ceros originales.
        // USBSTS.EINT=1 pero 0 eventos visibles es la firma exacta de este problema.
        // Solución: invalidar la línea de caché del TRB actual antes de leerlo.
        #[cfg(target_arch = "x86_64")]
        unsafe {
            // Cada TRB = 16 bytes. Una línea de caché = 64 bytes = 4 TRBs.
            // clflush invalida toda la línea, así que un solo flush es suficiente.
            _mm_clflush((self.seg.virt + off) as *const u8);
            _mm_lfence();
        }
        let c = self.seg.read_u32(off + 12);
        if (c & 1) == (self.cycle as u32) {
            Some(Trb {
                p: self.seg.read_u64(off),
                status: self.seg.read_u32(off + 8),
                ctrl: c,
            })
        } else {
            None
        }
    }

    fn pop(&mut self) -> Option<Trb> {
        let t = self.peek()?;
        self.deq = (self.deq + 1) % self.ntrb;
        if self.deq == 0 {
            self.cycle = !self.cycle;
        }
        Some(t)
    }

    fn erdp_phys(&self) -> u64 {
        (self.seg.phys + self.deq * 16) as u64
    }

    fn erst_phys(&self) -> u64 {
        self.erst.phys as u64
    }
}

const USB_CLASS_HID: u8 = 0x03;
const HID_SUBCLASS_BOOT: u8 = 0x01;
const HID_SUBCLASS_NONE: u8 = 0x00;
const HID_REQ_SET_PROTOCOL: u8 = 0x0b;
const HID_REQ_SET_IDLE: u8 = 0x0a;
const USB_DESC_IFACE: u8 = 0x04;
const USB_DESC_EP: u8 = 0x05;
const EP_TYPE_CONTROL: u32 = 4 << 3;
const EP_TYPE_INT_IN: u32 = 7 << 3;
const HID_PROTO_KEY: u8 = 1;
const HID_PROTO_MOUSE: u8 = 2;
const HID_PROTO_TABLET: u8 = 3;
const TABLET_RANGE: u32 = 32767;
const NO_MSI_VECTOR: usize = 0;

/// Number of TRBs (and report buffers) kept queued on every interrupt-IN HID
/// endpoint. A single in-flight TRB is fragile: if one transfer-completion
/// event is ever missed (event-ring race, lost MSI, idle-induced timer stall)
/// the endpoint goes silent forever — that's the "input dies after a few
/// seconds" symptom on real HW. Keeping a small ring of TRBs (each pointing at
/// its own buffer) means the controller can still satisfy several more
/// transfers before silence, and the resubmit on every event keeps the depth
/// constant. Buffers are tiny (≤ 64 B each) so the cost is negligible.
const HID_QUEUE_DEPTH: usize = 4;

/// Soft-recovery policy for an HCHalted controller. The driver used to set
/// `XHCI_HALTED` once and short-circuit `poll()` forever after, meaning a
/// single transient bus error killed input until reboot. Instead, the first
/// few times we observe HCHalted we now try a soft restart (clear sticky
/// USBSTS errors, write RS=1, wait briefly for HCH to drop). If that
/// recovers the controller, attempts reset; if every attempt fails we latch
/// `XHCI_HALTED` (genuinely dead silicon). Backoff stops the recovery from
/// hammering the controller on every io-wait iteration.
const MAX_HALT_RECOVERY_ATTEMPTS: u8 = 8;
const HALT_RECOVERY_BACKOFF_US: u64 = 500_000;
const HALT_RECOVERY_WAIT_US: u64 = 50_000;

pub struct XhciInner {
    pub mmio: XhciMmio,
    cmd: CmdRing,
    ev: EventRing,
    dcbaa: DmaBuf,
    pub max_slots: u8,
    pub max_ports: u8,
    context_size: usize,
    pub msi_vector: usize,
    slot_speed: Vec<u8>,
    slot_port: Vec<u8>,
    dev_ctx: Vec<Option<DmaBuf>>,
    xfer_rings: Vec<Option<XferRing>>,
    scratch_tbl: Option<DmaBuf>,
    scratch_pages: Vec<DmaBuf>,
    hids: Vec<HidDev>,
    pub fb_width: u32,
    pub fb_height: u32,
    /// Cambios de puerto diferidos para evitar re-entrada recursiva en pop_ev.
    pending_port_changes: Vec<u8>,
    /// HID interrupt endpoints (slot, dci) that completed a transfer with an
    /// error (Stall/Babble/…) and need a Reset Endpoint + re-arm. Deferred for
    /// the same reason as `pending_port_changes`: the recovery issues commands
    /// and waits on the event ring, which must not run nested inside `pop_ev`.
    pending_ep_resets: Vec<(u8, u8)>,
    /// HID enumeration deferred from PCI probe so boot can pass 80% quickly.
    boot_enum_pending: bool,
    /// Number of consecutive soft-recovery attempts since the controller last
    /// responded normally. Reset to 0 on every successful, non-halted poll.
    halt_attempts: u8,
    /// Monotonic timestamp (µs) of the last soft-recovery attempt; used to
    /// back off so we don't bang on the controller every io-wait iteration.
    halt_last_attempt_us: u64,
}

struct HidDev {
    slot_id: u8,
    port_id: u8,
    ep_dci: u8,
    ring_idx: usize,
    protocol: u8,
    report_len: usize,
    /// Round-robin ring of report buffers, one per pre-queued TRB on this
    /// endpoint (see [`HID_QUEUE_DEPTH`]). The controller fills them in
    /// enqueue order; we drain in the same order.
    bufs: Vec<DmaBuf>,
    /// Index into `bufs` of the next buffer whose transfer event will be
    /// dispatched. Advances after every consumed completion.
    dispatch_idx: usize,
    /// Index into `bufs` of the next buffer to be re-armed with a fresh TRB.
    /// Advances after every resubmit.
    enqueue_idx: usize,
    last_mods: u8,
    last_keys: [u8; 6],
    tab_x: u16,
    tab_y: u16,
    tab_init: bool,
}

impl XhciInner {
    fn new(mmio: XhciMmio, max_slots: u8, max_ports: u8, msi_vector: usize) -> DeviceResult<Self> {
        let hcc = mmio.read_cap(0x10);
        let context_size = if ((hcc >> 2) & 1) != 0 { 64 } else { 32 };
        let ns = max_slots as usize + 1;
        let dcbaa = DmaBuf::new(ns * 8, 4096)?;
        let mut dev_ctx = Vec::with_capacity(ns);
        dev_ctx.resize_with(ns, || None);
        let nr = ns * 32;
        let mut xfer_rings = Vec::with_capacity(nr);
        xfer_rings.resize_with(nr, || None);
        Ok(Self {
            mmio,
            cmd: CmdRing::new(64)?,
            ev: EventRing::new(256)?,
            dcbaa,
            max_slots,
            max_ports,
            context_size,
            msi_vector,
            slot_speed: alloc::vec![0; ns],
            slot_port: alloc::vec![0; ns],
            dev_ctx,
            xfer_rings,
            scratch_tbl: None,
            scratch_pages: Vec::new(),
            hids: Vec::new(),
            fb_width: 1024,
            fb_height: 768,
            pending_port_changes: Vec::new(),
            pending_ep_resets: Vec::new(),
            boot_enum_pending: true,
            halt_attempts: 0,
            halt_last_attempt_us: 0,
        })
    }

    fn ri(slot: u8, dci: u8) -> usize {
        slot as usize * 32 + dci as usize
    }

    fn pop_ev(&mut self, lis: Option<&EventListener<InputEvent>>) -> Option<Trb> {
        if let Some(trb) = self.ev.pop() {
            let etype = (trb.ctrl >> 10) & 0x3f;
            let erdp = self.ev.erdp_phys();
            self.mmio.write_rt64(0x38, (erdp as u64 & !0xf) | 0x8);

            if etype == 32 {
                // TRB_EVT_TRANSFER. Always run the transfer-side handler, even
                // during enumeration (lis=None from wait_ep0*): it re-arms the
                // completed HID interrupt endpoint (advance dequeue + resubmit)
                // and only *dispatches* a report when `lis` is Some. Skipping it
                // for lis=None (the old behaviour) left an already-enumerated
                // keyboard/mouse endpoint's ring un-advanced whenever a report
                // landed mid-enumeration of another device — a permanent 1-slot
                // desync per drop. Control (EP0, dci=1) transfer events match no
                // HID endpoint, so the handler is a no-op for them and the EP0
                // waiter still receives the TRB.
                self.handle_hid_transfer_side(&trb, lis);
            } else if etype == 34 {
                // TRB_EVT_PORT_STATUS: diferir para evitar re-entrada recursiva
                // durante la enumeración (pop_ev -> try_port_hid -> pop_ev).
                let port_id = ((trb.p >> 24) & 0xff) as u8;
                if port_id >= 1 && !self.pending_port_changes.contains(&port_id) {
                    self.pending_port_changes.push(port_id);
                }
            }

            return Some(trb);
        }
        None
    }

    /// Procesar cambios de puerto diferidos. Llamar solo desde contextos no-reentrantes
    /// (process_irq_events, poll, enumerate_root_hid tras cada puerto).
    fn drain_pending_port_changes(&mut self) {
        let ports: Vec<u8> = self.pending_port_changes.drain(..).collect();
        for port_id in ports {
            let _ = self.handle_port_status_change(port_id);
        }
    }

    fn resubmit_hid_normal_trb(
        &mut self,
        ring_idx: usize,
        buf_phys: u64,
        len: u16,
        slot: u8,
        ep: u8,
    ) {
        let trb = trb_normal(buf_phys, len, true);
        if let Some(r) = self.xfer_rings.get_mut(ring_idx).and_then(|o| o.as_mut()) {
            match r.push(trb) {
                Ok(_) => {
                    fence(Ordering::SeqCst);
                    self.mmio.ring_db(slot, ep);
                }
                Err(_) => {
                    r.advance_dequeue(1);
                    match r.push(trb_normal(buf_phys, len, true)) {
                        Ok(_) => {
                            fence(Ordering::SeqCst);
                            self.mmio.ring_db(slot, ep);
                        }
                        Err(_) => {}
                    }
                }
            }
        }
    }

    fn handle_hid_transfer_side(
        &mut self,
        ev: &Trb,
        lis: Option<&EventListener<InputEvent>>,
    ) -> bool {
        let ty = (ev.ctrl >> 10) & 0x3f;
        if ty != 32 {
            // TRB_EVT_TRANSFER
            return false;
        }
        let i = ((ev.ctrl >> 24) & 0xff) as usize; // slot
        let dci = ((ev.ctrl >> 16) & 0x1f) as u8;
        let cc = (ev.status >> 24) & 0xff;

        if i == 0 || i > self.max_slots as usize {
            return false;
        }

        if cc != TRB_CC_SUCCESS && cc != TRB_CC_SHORT {
            // Transfer error. Stall (0x0d) halts the endpoint outright; Babble /
            // Transaction Error can too. Doing nothing (the old behaviour) left
            // the endpoint dead until the user unplugged and replugged the
            // device. Skip the failed TRB and queue a deferred Reset Endpoint +
            // re-arm (commands must not run nested inside this event drain).
            if let Some(idx) = self
                .hids
                .iter()
                .position(|h| h.slot_id == i as u8 && h.ep_dci == dci)
            {
                let ridx = self.hids[idx].ring_idx;
                if let Some(r) = self.xfer_rings.get_mut(ridx).and_then(|o| o.as_mut()) {
                    r.advance_dequeue(1);
                }
                // No report dispatched for the errored transfer: advance the
                // dispatch head too, keeping it in lockstep with the enqueue
                // head (which the deferred re-arm advances).
                self.hids[idx].dispatch_idx = (self.hids[idx].dispatch_idx + 1) % HID_QUEUE_DEPTH;
                let key = (i as u8, dci);
                if !self.pending_ep_resets.contains(&key) {
                    self.pending_ep_resets.push(key);
                }
            }
            return false;
        }

        let hid_i = self
            .hids
            .iter()
            .position(|h| h.slot_id == i as u8 && h.ep_dci == dci);

        let idx = match hid_i {
            Some(idx) => idx,
            None => return false,
        };

        if lis.is_none() {}
        let (ridx, blen, buf_phys) = {
            let h = &self.hids[idx];
            // Re-arm with the buffer at the enqueue head of the round-robin
            // ring (its TRB is the one we're about to push back into the
            // ring). dispatch_hid drains from a separate head so reads see
            // the report whose event we're processing, not the one the
            // controller might be writing next.
            let buf_phys = h.bufs[h.enqueue_idx].sub_phys(0);
            (h.ring_idx, (h.report_len.min(64)) as u16, buf_phys)
        };
        if let Some(l) = lis {
            self.dispatch_hid(idx, l);
        }
        if let Some(r) = self.xfer_rings.get_mut(ridx).and_then(|o| o.as_mut()) {
            r.advance_dequeue(1);
        }
        self.resubmit_hid_normal_trb(ridx, buf_phys, blen, i as u8, dci);
        if let Some(h) = self.hids.get_mut(idx) {
            h.enqueue_idx = (h.enqueue_idx + 1) % HID_QUEUE_DEPTH;
        }
        true
    }

    fn wait_cmd_phys(&mut self, cmd_trb_phys: u64) -> DeviceResult<()> {
        let timeout_us = 5_000_000;
        let start = timer_now_us();
        let mut spins = 0u64;
        while !xhci_wait_expired(start, timeout_us, spins) {
            // 5s
            if let Some(ev) = self.pop_ev(None) {
                let ty = (ev.ctrl >> 10) & 0x3f;
                if ty == 33 {
                    // TRB_EVT_CMD_COMP
                    let match_addr = ev.p == cmd_trb_phys;
                    if match_addr {
                        let cc = (ev.status >> 24) & 0xff;
                        if cc == TRB_CC_SUCCESS || cc == TRB_CC_SHORT {
                            return Ok(());
                        }
                        warn!("[xhci] comando falló con CC={}", cc);
                        return Err(DeviceError::IoError);
                    }
                }
            }
            spins = spins.saturating_add(1);
            spin_loop();
        }
        if spins >= xhci_wait_spin_limit(timeout_us) {
            warn!("[xhci] wait_cmd_phys salió por guard de spins (timer estancado?)");
        }
        error!("[xhci] timeout esperando comando");
        Err(DeviceError::IoError)
    }

    fn wait_cmd_phys_slot(&mut self, cmd_trb_phys: u64) -> DeviceResult<u8> {
        let timeout_us = 5_000_000;
        let start = timer_now_us();
        let mut spins = 0u64;
        while !xhci_wait_expired(start, timeout_us, spins) {
            // 5s
            if let Some(ev) = self.pop_ev(None) {
                let ty = (ev.ctrl >> 10) & 0x3f;
                if ty == 33 {
                    // TRB_EVT_CMD_COMP
                    let match_addr = ev.p == cmd_trb_phys;
                    let slot_id = (ev.ctrl >> 24) & 0xff;
                    if match_addr {
                        let cc = (ev.status >> 24) & 0xff;
                        if cc != TRB_CC_SUCCESS && cc != TRB_CC_SHORT {
                            warn!("[xhci] comando de slot falló con CC={}", cc);
                            return Err(DeviceError::IoError);
                        }
                        return Ok(slot_id as u8);
                    }
                }
            }
            spins = spins.saturating_add(1);
            spin_loop();
        }
        if spins >= xhci_wait_spin_limit(timeout_us) {
            warn!("[xhci] wait_cmd_phys_slot salió por guard de spins (timer estancado?)");
        }
        error!("[xhci] timeout esperando comando de slot");
        Err(DeviceError::IoError)
    }

    fn exec_cmd(&mut self, t: Trb) -> DeviceResult<()> {
        let p = self.cmd.push(t)?;
        self.mmio.ring_db(0, 0);
        self.wait_cmd_phys(p)
    }

    fn reset_endpoint_and_dequeue(&mut self, slot: u8, dci: u8) -> DeviceResult<()> {
        info!(
            "[xhci] reset_endpoint_and_dequeue slot={} dci={}",
            slot, dci
        );
        let p_reset = self.cmd.push(trb_reset_endpoint(slot, dci))?;
        self.mmio.ring_db(0, 0);
        if let Err(e) = self.wait_cmd_phys(p_reset) {
            warn!("[xhci] reset endpoint command failed: {:?}", e);
        }

        let ri = Self::ri(slot, dci);
        if let Some(ring) = self.xfer_rings.get(ri).and_then(|o| o.as_ref()) {
            let deq_phys = ring.deq_phys();
            let dcs = ring.deq_cycle();
            info!(
                "[xhci] set tr dequeue pointer: deq_phys={:#x} dcs={}",
                deq_phys, dcs
            );
            let p_deq = self
                .cmd
                .push(trb_set_tr_dequeue_pointer(deq_phys, dcs, slot, dci))?;
            self.mmio.ring_db(0, 0);
            if let Err(e) = self.wait_cmd_phys(p_deq) {
                warn!("[xhci] set tr dequeue pointer command failed: {:?}", e);
            }
        }
        Ok(())
    }

    fn wait_ep0_status_any(
        &mut self,
        slot: u8,
        setup_phys: u64,
        data_phys: u64,
        status_phys: u64,
        n_trb: usize,
        timeout_us: u64,
    ) -> DeviceResult<()> {
        let speed = self.slot_speed[slot as usize];
        info!(
            "[xhci] EP0 slot={} speed={} esperando: setup={:#x} data={:#x} status={:#x}",
            slot, speed, setup_phys, data_phys, status_phys
        );
        // Calcular el rango del anillo EP0 para este slot para hacer matching flexible.
        // VirtualBox a veces reporta ev.p apuntando al TRB siguiente al completado,
        // así que comprobamos si p cae en la ventana [setup_phys, status_phys+16).
        let lo = setup_phys.min(data_phys).min(status_phys);
        let hi = setup_phys.max(data_phys).max(status_phys) + 16;
        let start = timer_now_us();
        let mut spins = 0u64;
        let mut ev_count = 0u32;
        while !xhci_wait_expired(start, timeout_us, spins) {
            if let Some(ev) = self.pop_ev(None) {
                let ty = (ev.ctrl >> 10) & 0x3f;
                ev_count += 1;
                if ty == 32 {
                    let ev_slot = ((ev.ctrl >> 24) & 0xff) as u8;
                    let ev_dci = ((ev.ctrl >> 16) & 0x1f) as u8;
                    let cc = (ev.status >> 24) & 0xff;
                    info!(
                        "[xhci] EP0 ev#{}: type=Transfer slot={} dci={} p={:#x} CC={}",
                        ev_count, ev_slot, ev_dci, ev.p, cc
                    );
                    // Matching flexible: dirección exacta (QEMU) o dentro del rango de la
                    // transferencia de control EP0 (VirtualBox puede reportar TRB+N).
                    let in_range = ev.p == setup_phys
                        || ev.p == data_phys
                        || ev.p == status_phys
                        || (ev.p >= lo && ev.p < hi);
                    if ev_slot == slot && in_range {
                        let i = Self::ri(slot, 1);
                        if let Some(r) = self.xfer_rings.get_mut(i).and_then(|o| o.as_mut()) {
                            r.advance_dequeue(n_trb);
                        }
                        // CC=1 Success, CC=13 Short Packet: ambos son éxito para Control.
                        // CC=6 (TRB Error) es también success en Status Stage de algunos HCs
                        // (VirtualBox, algunos Intel xHCI físicos).
                        if cc == TRB_CC_SUCCESS || cc == TRB_CC_SHORT {
                            info!("[xhci] EP0 slot={} OK (CC={})", slot, cc);
                            return Ok(());
                        }
                        if cc == 6 {
                            info!(
                                "[xhci] EP0 slot={} STALL (CC=6). Clearing halt, returning OK.",
                                slot
                            );
                            let _ = self.reset_endpoint_and_dequeue(slot, 1);
                            return Ok(());
                        }
                        warn!(
                            "[xhci] EP0 slot={} error CC={}. Intentando reset_endpoint.",
                            slot, cc
                        );
                        let _ = self.reset_endpoint_and_dequeue(slot, 1);
                        return Err(DeviceError::IoError);
                    } else {
                        // Evento descartado — diagnosticar por qué
                        if ev_slot != slot {
                            // evento de otro slot, normal durante enumeración concurrente
                        } else {
                            warn!("[xhci] EP0 slot={} ev p={:#x} FUERA de rango", slot, ev.p);
                        }
                    }
                } else {
                    info!("[xhci] EP0 espera: ev#{} type={}", ev_count, ty);
                }
            }
            spins = spins.saturating_add(1);
            spin_loop();
        }
        if spins >= xhci_wait_spin_limit(timeout_us) {
            warn!(
                "[xhci] EP0 slot={} salió por guard de spins (timer estancado?)",
                slot
            );
        }
        error!(
            "[xhci] timeout EP0 slot={} ({} eventos vistos, setup={:#x})",
            slot, ev_count, setup_phys
        );
        let sts = self.mmio.read_op(4);
        error!("[xhci] USBSTS={:#010x}", sts);
        let _ = self.reset_endpoint_and_dequeue(slot, 1);
        Err(DeviceError::IoError)
    }

    fn ep0_control_in(&mut self, slot: u8, setup: Trb, buf: &DmaBuf, len: u32) -> DeviceResult<()> {
        let i = Self::ri(slot, 1);
        // Diagnóstico: estado del anillo EP0 antes de pushear TRBs
        if let Some(r) = self.xfer_rings.get(i).and_then(|o| o.as_ref()) {
            warn!(
                "[xhci] ep0_ctrl_in slot={} ring.phys={:#x} enq={} cycle={} cap={}",
                slot, r.buf.phys, r.enq, r.cycle as u8, r.cap
            );
        }
        let (setup_phys, data_phys, status_phys) = {
            let ring = self
                .xfer_rings
                .get_mut(i)
                .and_then(|o| o.as_mut())
                .ok_or(DeviceError::NotSupported)?;
            let st = trb_data(buf.sub_phys(0), len, true); // Data Stage is IN (true)
            let su = trb_status(false, true); // Status Stage is OUT (false)
            let p1 = ring.push(setup)?;
            let p2 = ring.push(st)?;
            let p3 = ring.push(su)?;
            (p1, p2, p3)
        };
        // Flush the transfer ring so the controller sees the TRBs before we ring the doorbell.
        if let Some(r) = self.xfer_rings.get(i).and_then(|o| o.as_ref()) {
            r.buf.flush(0, r.buf.len);
        }
        self.mmio.ring_db(slot, 1);
        self.wait_ep0_status_any(slot, setup_phys, data_phys, status_phys, 3, 2_000_000)
    }

    fn ep0_control_out0(&mut self, slot: u8, setup: Trb, status_in: bool) -> DeviceResult<()> {
        let i = Self::ri(slot, 1);
        let (setup_phys, status_phys) = {
            let ring = self
                .xfer_rings
                .get_mut(i)
                .and_then(|o| o.as_mut())
                .ok_or(DeviceError::NotSupported)?;
            let su = trb_status(status_in, true);
            let p1 = ring.push(setup)?;
            let p2 = ring.push(su)?;
            (p1, p2)
        };
        if let Some(r) = self.xfer_rings.get(i).and_then(|o| o.as_ref()) {
            r.buf.flush(0, r.buf.len);
        }
        self.mmio.ring_db(slot, 1);
        self.wait_ep0_status_any(slot, setup_phys, setup_phys, status_phys, 2, 2_000_000)
    }

    /// Igual que ep0_control_out0 pero con timeout corto (200ms) para comandos opcionales
    /// HID como SET_IDLE o SET_PROTOCOL que algunos dispositivos no responden.
    fn ep0_control_out0_optional(
        &mut self,
        slot: u8,
        setup: Trb,
        status_in: bool,
    ) -> DeviceResult<()> {
        let i = Self::ri(slot, 1);
        let (setup_phys, status_phys) = {
            let ring = self
                .xfer_rings
                .get_mut(i)
                .and_then(|o| o.as_mut())
                .ok_or(DeviceError::NotSupported)?;
            let su = trb_status(status_in, true);
            let p1 = ring.push(setup)?;
            let p2 = ring.push(su)?;
            (p1, p2)
        };
        if let Some(r) = self.xfer_rings.get(i).and_then(|o| o.as_ref()) {
            r.buf.flush(0, r.buf.len);
        }
        self.mmio.ring_db(slot, 1);
        // 200ms: suficiente para dispositivos lentos, no bloquea si el dispositivo
        // no responde (SET_IDLE es opcional en USB HID spec).
        self.wait_ep0_status_any(slot, setup_phys, setup_phys, status_phys, 2, 200_000)
    }

    pub fn reset_and_run(&mut self) -> DeviceResult<()> {
        let m = &self.mmio;
        m.perform_bios_handoff();

        // 1. Detener el controlador si está corriendo (spec §4.2.1)
        let usbcmd = m.read_op(0);
        if (usbcmd & 1) != 0 {
            info!("[xhci] deteniendo controlador antes del reset");
            m.write_op(0, usbcmd & !1); // RS=0
            let timeout_us = 100_000;
            let start = timer_now_us();
            let mut spins = 0u64;
            while (m.read_op(4) & 1) == 0 && !xhci_wait_expired(start, timeout_us, spins) {
                spins = spins.saturating_add(1);
                spin_loop();
            }
            if spins >= xhci_wait_spin_limit(timeout_us) {
                warn!("[xhci] stop-before-reset salió por guard de spins (timer estancado?)");
            }
        }

        // 2. Esperar a que CNR (Controller Not Ready) se limpie antes del reset
        let timeout_us = 1_000_000;
        let start = timer_now_us();
        let mut spins = 0u64;
        while (m.read_op(4) & (1 << 11)) != 0 && !xhci_wait_expired(start, timeout_us, spins) {
            spins = spins.saturating_add(1);
            spin_loop();
        }
        if spins >= xhci_wait_spin_limit(timeout_us) {
            warn!("[xhci] wait-CNR-before-reset salió por guard de spins (timer estancado?)");
        }
        if (m.read_op(4) & (1 << 11)) != 0 {
            error!("[xhci] timeout esperando CNR antes de reset");
            return Err(DeviceError::NotReady);
        }

        // 3. Emitir HCRST (bit 1 de USBCMD)
        info!("[xhci] emitiendo HCRST");
        m.write_op(0, m.read_op(0) | 2);
        let timeout_us = 100_000;
        let start = timer_now_us();
        let mut spins = 0u64;
        while (m.read_op(0) & 2) != 0 && !xhci_wait_expired(start, timeout_us, spins) {
            spins = spins.saturating_add(1);
            spin_loop();
        }
        if spins >= xhci_wait_spin_limit(timeout_us) {
            warn!("[xhci] wait-HCRST-clear salió por guard de spins (timer estancado?)");
        }

        // 4. Esperar a que CNR se limpie de nuevo tras reset
        let timeout_us = 1_000_000;
        let start = timer_now_us();
        let mut spins = 0u64;
        while (m.read_op(4) & (1 << 11)) != 0 && !xhci_wait_expired(start, timeout_us, spins) {
            spins = spins.saturating_add(1);
            spin_loop();
        }
        if spins >= xhci_wait_spin_limit(timeout_us) {
            warn!("[xhci] wait-CNR-after-reset salió por guard de spins (timer estancado?)");
        }
        if (m.read_op(4) & (1 << 11)) != 0 {
            error!("[xhci] timeout esperando CNR tras reset");
            return Err(DeviceError::NotReady);
        }
        info!("[xhci] reset completado");

        // Configurar Scratchpad buffers si son necesarios
        let hcsp2 = m.read_cap(8);
        let sb_lo = (hcsp2 >> 27) & 0x1f;
        let sb_hi = (hcsp2 >> 21) & 0x1f;
        let sb = sb_lo | (sb_hi << 5);
        if sb > 0 {
            info!("[xhci] reservando {} scratchpad buffers", sb);
            let tbl = DmaBuf::new(sb as usize * 8, 64)?;
            for i in 0..sb as usize {
                let pg = DmaBuf::new(PAGE_SIZE, PAGE_SIZE)?;
                tbl.write_u64(i * 8, pg.sub_phys(0));
                self.scratch_pages.push(pg);
            }
            self.dcbaa.write_u64(0, tbl.sub_phys(0));
            self.scratch_tbl = Some(tbl);
        }

        // Configurar Max Slots y bases de datos
        let cfg = m.read_op(0x38);
        m.write_op(0x38, (cfg & !0xff) | self.max_slots as u32);

        m.write_op64(0x30, self.dcbaa.phys as u64);

        let crcr = self.cmd.crcr();
        m.write_op64(0x18, (crcr as u64 & !0x3F) | 1);

        // Configurar Interrupter 0 del Event Ring.
        // Orden mandatorio por spec xHCI §5.5.2:
        //   1. Limpiar IP en IMAN (offset 0x20)
        //   2. Poner IMOD (offset 0x24)
        //   3. Escribir ERSTSZ (offset 0x28) = número de segmentos (1)
        //   4. Escribir ERSTBA (offsets 0x30/0x34)
        //   5. Escribir ERDP (offsets 0x38/0x3C)
        m.write_rt(0x20, 1); // IMAN: limpiar IP (bit 0), IE=0 de momento
        m.write_rt(0x24, 0); // IMOD=0 (máxima respuesta, sin moderación)
        m.write_rt(0x28, 1); // ERSTSZ = 1 segmento
        m.write_rt64(0x30, self.ev.erst_phys() as u64);
        let erdp = self.ev.erdp_phys();
        m.write_rt64(0x38, (erdp as u64 & !0xf) | 8);

        if self.msi_vector > 0 {
            m.write_rt(0x20, 3); // IMAN: IE=1, IP=1 (habilitar interrupciones)
        }

        // Iniciar controlador (Run)
        let mut usbcmd = m.read_op(0);
        usbcmd |= 1; // RS=1
        if self.msi_vector > 0 {
            usbcmd |= 1 << 2; // INTE
        }
        m.write_op(0, usbcmd);

        // Esperar a que el controlador salga de HCHalted (bit 0 de USBSTS=1 = halted).
        // NOTA: la condición es sts & 1 != 0 — el controlador está DETENIDO mientras ese bit esté en 1.
        let timeout_us = 100_000;
        let start = timer_now_us();
        let mut spins = 0u64;
        while !xhci_wait_expired(start, timeout_us, spins) {
            let sts = m.read_op(4);
            if (sts & 1) == 0 {
                // HCHalted=0 → controlador corriendo
                break;
            }
            spins = spins.saturating_add(1);
            spin_loop();
        }
        if spins >= xhci_wait_spin_limit(timeout_us) {
            warn!("[xhci] wait-HCHalted-clear salió por guard de spins (timer estancado?)");
        }
        {
            let sts = m.read_op(4);
            if (sts & 1) != 0 {
                error!(
                    "[xhci] controlador no arrancó (HCHalted persiste), USBSTS={:#010x}",
                    sts
                );
                self.dump_halt_diagnostics();
                return Err(DeviceError::NotReady);
            }
        }
        info!("[xhci] controlador en marcha");

        // Energía de puertos (Port Power, PP = bit 9).
        // Se usa PORTSC_RW1C_AND_RO_MASK para nunca escribir 1 en PED (bit 1, RW1C) ni en
        // los bits de cambio, lo que borraría accidentalmente la habilitación del puerto.
        for p in 1..=self.max_ports {
            let off = 0x400 + (p as usize - 1) * 0x10;
            let sc = m.read_op(off);
            if (sc & (1 << 9)) == 0 {
                info!("[xhci] encendiendo puerto {} (PP=0 → 1)", p);
                m.write_op(off, (sc & !PORTSC_RW1C_AND_RO_MASK) | (1 << 9));
            }
        }
        // Pequeña espera tras dar energía (USB spec exige ≥100ms de VBUS estable antes de
        // que el dispositivo pueda responder; los devices gaming con firmware complejo lo necesitan).
        xhci_spin_delay_us(100_000);

        Ok(())
    }

    fn enumerate_root_hid(&mut self) {
        let maxp = self.max_ports;
        for port in 1..=maxp {
            if let Err(_e) = self.try_port_hid(port) {}
            // Procesar cambios de estado pendientes entre puertos para no acumularlos.
            self.drain_pending_port_changes();
        }
    }

    fn wait_port_ready(&self, off: usize, require_pr_clear: bool) -> Option<u8> {
        let m = &self.mmio;
        // Require multiple consecutive "ready" samples to filter transient link-state flaps.
        const STABLE_SAMPLES: u32 = 5;
        let mut stable = 0u32;
        let timeout_us = 1_000_000;
        let start = timer_now_us();
        let mut spins = 0u64;
        while !xhci_wait_expired(start, timeout_us, spins) {
            // Max 1s
            let s = m.read_op(off);
            let ccs = (s & 1) != 0;
            let ped = (s & (1 << 1)) != 0;
            let pr = (s & (1 << 4)) != 0;
            let spd = ((s >> 10) & 0x0f) as u8;
            let ready = ccs && spd != 0 && (ped || spd >= 4) && (!require_pr_clear || !pr);
            if ready {
                stable = stable.saturating_add(1);
                if stable >= STABLE_SAMPLES {
                    return Some(spd);
                }
            } else {
                stable = 0;
            }
            spins = spins.saturating_add(1);
            xhci_spin_delay_us(1000);
        }
        None
    }

    fn try_port_hid(&mut self, port: u8) -> DeviceResult<()> {
        let off = 0x400 + (port as usize - 1) * 0x10;
        let mut portsc = self.mmio.read_op(off);
        if (portsc & 1) == 0 {
            return Ok(());
        }
        if self
            .slot_port
            .iter()
            .enumerate()
            .skip(1)
            .any(|(_, &p)| p == port)
        {
            self.cleanup_port(port)?;
            portsc = self.mmio.read_op(off);
            if (portsc & 1) == 0 {
                return Ok(());
            }
        }
        // Ensure port power if the controller reports it as off.
        if (portsc & (1 << 9)) == 0 {
            info!("[xhci] puerto {}: PP=0, encendiendo", port);
            self.mmio
                .write_op(off, (portsc & !PORTSC_RW1C_AND_RO_MASK) | (1 << 9));
            xhci_spin_delay_us(100_000);
            portsc = self.mmio.read_op(off);
        }
        let pre_spd = ((portsc >> 10) & 0x0f) as u8;
        // needs_reset: solo si no hay velocidad asignada o el enlace no está habilitado.
        // spd >= 4 = SuperSpeed: PED no aplica de la misma forma; el port ya está listo.
        let ped = (portsc & (1 << 1)) != 0;
        let needs_reset = pre_spd == 0 || (!ped && pre_spd <= 3);

        // Reset del puerto (PR=1)
        if needs_reset {
            info!("[xhci] puerto {}: emitiendo reset", port);
            self.mmio
                .write_op(off, (portsc & !PORTSC_RW1C_AND_RO_MASK) | (1 << 4));

            // Espera robusta de reset (100ms)
            let mut success = false;
            let timeout_us = 100_000;
            let start = timer_now_us();
            let mut spins = 0u64;
            while !xhci_wait_expired(start, timeout_us, spins) {
                let s = self.mmio.read_op(off);
                if (s & (1 << 21)) != 0 || (s & (1 << 4)) == 0 {
                    success = true;
                    break;
                }
                spins = spins.saturating_add(1);
                spin_loop();
            }
            if !success {
                warn!("[xhci] puerto {}: timeout en reset", port);
            }
        }

        let spd = self.wait_port_ready(off, needs_reset).unwrap_or_else(|| {
            let s = self.mmio.read_op(off);
            ((s >> 10) & 0x0f) as u8
        });
        portsc = self.mmio.read_op(off);

        // Limpiar bits de cambio (CSC, PRC, etc) escribiendo 1; conservar PP, PED, etc.
        let clr = PORTSC_CHANGE_BITS;
        self.mmio
            .write_op(off, (portsc & !PORTSC_RW1C_AND_RO_MASK) | clr);

        // Pequeño delay tras reset para estabilización del link
        xhci_spin_delay_us(10_000);

        if spd == 0 || (self.mmio.read_op(off) & 1) == 0 {
            return Ok(());
        }
        match self.setup_device(port, spd) {
            Ok(()) => Ok(()),
            Err(first_err) => {
                warn!(
                    "[xhci] puerto {}: primer intento de enumeración falló ({:?}), reintentando",
                    port, first_err
                );
                self.cleanup_port(port)?;
                let mut s = self.mmio.read_op(off);
                if (s & 1) == 0 {
                    return Ok(());
                }
                self.mmio
                    .write_op(off, (s & !PORTSC_RW1C_AND_RO_MASK) | (1 << 4));
                xhci_spin_delay_us(100_000);
                let spd_retry = self.wait_port_ready(off, true).unwrap_or_else(|| {
                    s = self.mmio.read_op(off);
                    ((s >> 10) & 0x0f) as u8
                });
                s = self.mmio.read_op(off);
                self.mmio
                    .write_op(off, (s & !PORTSC_RW1C_AND_RO_MASK) | clr);
                xhci_spin_delay_us(50_000);
                if spd_retry == 0 || (self.mmio.read_op(off) & 1) == 0 {
                    return Ok(());
                }
                match self.setup_device(port, spd_retry) {
                    Ok(()) => Ok(()),
                    Err(second_err) => {
                        let _ = self.cleanup_port(port);
                        Err(second_err)
                    }
                }
            }
        }
    }

    fn setup_device(&mut self, port: u8, speed: u8) -> DeviceResult<()> {
        let cmd_trb_phys = self.cmd.push(trb_enable_slot())?;
        self.mmio.ring_db(0, 0); // Doorbell 0: Comando
        let slot = self.wait_cmd_phys_slot(cmd_trb_phys)?;
        if slot == 0 {
            return Err(DeviceError::IoError);
        }
        self.slot_speed[slot as usize] = speed;
        self.slot_port[slot as usize] = port;

        let csz = self.context_size;
        let dev_sz = 32 * csz;
        let dev = DmaBuf::new(dev_sz, 64)?;
        // Flush DCBAA entry: el controlador lee este puntero vía DMA.
        // También hay que asegurarse de que el buffer de contexto esté en RAM (ceros).
        dev.flush(0, dev_sz);
        self.dcbaa.write_u64(slot as usize * 8, dev.sub_phys(0));
        self.dcbaa.flush(slot as usize * 8, 8);
        self.dev_ctx[slot as usize] = Some(dev);

        let input_sz = 33 * csz;
        let ic = DmaBuf::new(input_sz, 64)?;
        ic.write_u32(4, 0x03);
        let s0 = 1 * csz;
        // Slot Context DW0 (§6.2.2):
        //   bits [19: 0] Route String = 0 (root hub, no hub entre medias)
        //   bits [23:20] Speed        = PORTSC speed code
        //   bit  [26]    MTT          = 0 (sin Multi-Transaction Translator)
        //   bit  [27]    Hub          = 0 (no es un hub)
        //   bits [31:27] Ctx Entries  = 1 (solo EP0 inicial; se actualizará en Configure Endpoint)
        let slot_dw0 = ((speed as u32) << 20) | (1u32 << 27); // Speed + Context Entries=1
        ic.write_u32(s0, slot_dw0);
        ic.write_u32(s0 + 4, (port as u32) << 16);
        let ep0 = 2 * csz;
        // xHCI PORTSC speed: 1=FS 2=LS 3=HS 4=SS Gen1 5=SS Gen2 …
        // Both FS(speed=1) and LS(speed=2) devices must start EP0 at 8 bytes until the
        // device descriptor tells us the real bMaxPacketSize0. Using 64 here breaks
        // enumeration for common HID keyboards/mice that come up on USB 1.x/2.0.
        let mps: u32 = match speed {
            1 | 2 => 8,
            3 => 64,
            4..=6 => 512,
            _ => return Err(DeviceError::InvalidParam),
        };
        ic.write_u32(ep0 + 4, (3 << 1) | EP_TYPE_CONTROL | (mps << 16));
        let ep0_ring = XferRing::new(32)?;
        let ep0_phys = ep0_ring.ring_phys();
        ic.write_u64(ep0 + 8, ep0_phys);
        ic.flush(0, input_sz);
        let ri = Self::ri(slot, 1);

        warn!(
            "[xhci] setup slot={} port={} speed={} csz={}",
            slot, port, speed, csz
        );
        warn!(
            "[xhci]   dcbaa[{}]={:#x}",
            slot,
            self.dcbaa.read_u64(slot as usize * 8)
        );
        warn!(
            "[xhci]   ic phys={:#x} ep0_ring_phys={:#x}",
            ic.sub_phys(0),
            ep0_phys
        );
        warn!(
            "[xhci]   ic.DW1(add)={:#010x} SlotCtx_DW0={:#010x} SlotCtx_DW1={:#010x}",
            ic.read_u32(4),
            ic.read_u32(s0),
            ic.read_u32(s0 + 4)
        );
        warn!(
            "[xhci]   EP0Ctx_DW1={:#010x} EP0_TRDeqPtr={:#x}",
            ic.read_u32(ep0 + 4),
            ic.read_u64(ep0 + 8)
        );

        self.xfer_rings[ri] = Some(ep0_ring);

        let p2 = self.cmd.push(trb_address_device(ic.sub_phys(0), slot))?;
        self.mmio.ring_db(0, 0);
        self.wait_cmd_phys(p2)?;
        // ic must stay alive until after wait_cmd_phys: xHC reads it asynchronously via DMA.
        // Rust would drop it here after the semicolon, which is correct (after wait completes).
        let _ = &ic; // force ic to live until this point

        warn!(
            "[xhci] Address Device completado slot={} USBSTS={:#010x}",
            slot,
            self.mmio.read_op(4)
        );

        // Pequeña pausa tras Address Device: algunos dispositivos FS/LS necesitan
        // tiempo para procesar el SET_ADDRESS y estar listos en la nueva dirección.
        {
            xhci_spin_delay_us(2_000); // 2ms
        }

        // Invalidar la caché del descriptor buffer antes de pasarlo al controlador
        // (el controlador escribirá en él via DMA; queremos ver los datos frescos).
        let desc = DmaBuf::new(64, 64)?;
        desc.flush(0, 64);
        warn!(
            "[xhci] GET_DESCRIPTOR slot={} desc.phys={:#x} ep0_ring.phys={:#x}",
            slot,
            desc.phys,
            self.xfer_rings
                .get(ri)
                .and_then(|o| o.as_ref())
                .map(|r| r.buf.phys)
                .unwrap_or(0)
        );
        self.ep0_control_in(slot, trb_setup(0x80, 0x06, 0x0100, 0, 18, 3), &desc, 18)?;

        // Invalidar caché del buffer de descriptor para ver los datos escritos por DMA.
        desc.flush(0, 64);

        // Leer bMaxPacketSize0 (byte 7) y actualizar contexto de EP0
        let mut raw_desc = [0u8; 18];
        desc.read_into(0, &mut raw_desc);
        let vid = u16::from_le_bytes([raw_desc[8], raw_desc[9]]);
        let pid = u16::from_le_bytes([raw_desc[10], raw_desc[11]]);
        info!(
            "[xhci] puerto {}: dispositivo detectado VID={:04x} PID={:04x} class={:02x}",
            port, vid, pid, raw_desc[4]
        );

        let real_mps = raw_desc[7] as u32;
        if real_mps != mps && real_mps >= 8 {
            let ic_upd = DmaBuf::new(input_sz, 64)?;
            ic_upd.write_u32(4, 0x03); // Add Slot (A0) and EP0 (A1)
                                       // Copiar contexto actual
            if let Some(dev_ctx) = self.dev_ctx[slot as usize].as_ref() {
                // Invalidar caché antes de leer datos escritos por el controlador via DMA.
                dev_ctx.flush(0, dev_sz);
                // Copiar Slot Context (dev index 0 -> input index 1)
                for i in 0..(csz / 4) {
                    ic_upd.write_u32(csz + i * 4, dev_ctx.read_u32(i * 4));
                }
                // Copiar EP0 Context (dev index 1 -> input index 2)
                for i in 0..(csz / 4) {
                    ic_upd.write_u32(2 * csz + i * 4, dev_ctx.read_u32(csz + i * 4));
                }
            }
            // Actualizar MPS en el contexto de EP0
            let ep0_dw1 = ic_upd.read_u32(ep0 + 4);
            ic_upd.write_u32(ep0 + 4, (ep0_dw1 & 0x0000FFFF) | (real_mps << 16));

            let p_upd = self
                .cmd
                .push(trb_configure_endpoint(ic_upd.sub_phys(0), slot))?;
            ic_upd.flush(0, input_sz);
            self.mmio.ring_db(0, 0);
            let _ = self.wait_cmd_phys(p_upd);
        }

        self.setup_hid_from_config(slot, csz, port)?;

        Ok(())
    }

    fn setup_hid_from_config(&mut self, slot: u8, csz: usize, port: u8) -> DeviceResult<()> {
        let sniff = DmaBuf::new(64, 64)?;
        sniff.flush(0, 64); // evict stale zeros before DMA
        self.ep0_control_in(slot, trb_setup(0x80, 0x06, 0x0200, 0, 9, 3), &sniff, 9)?;
        sniff.flush(0, 64); // invalidate so CPU reads fresh DMA data
        let mut hdr = [0u8; 9];
        sniff.read_into(0, &mut hdr);
        let total = u16::from_le_bytes([hdr[2], hdr[3]]) as usize;
        if total < 9 || total > 8192 {
            return Err(DeviceError::InvalidParam);
        }
        let buf_len = ((total + 63) / 64 * 64).max(64);
        let cfgb = DmaBuf::new(buf_len, 64)?;
        cfgb.flush(0, buf_len); // evict stale zeros before DMA
        self.ep0_control_in(
            slot,
            trb_setup(0x80, 0x06, 0x0200, 0, total as u16, 3),
            &cfgb,
            total as u32,
        )?;
        cfgb.flush(0, buf_len); // invalidate so CPU reads fresh DMA data

        let mut raw = alloc::vec![0u8; total];
        cfgb.read_into(0, &mut raw[..total]);
        info!(
            "[xhci] Configuration Descriptor slot={} bytes: {:?}",
            slot, raw
        );
        let config_val = raw.get(5).copied().unwrap_or(1).max(1);

        // SET_CONFIGURATION
        let _ = self.ep0_control_out0(
            slot,
            trb_setup(0x00, 0x09, config_val as u16, 0, 0, 0),
            true,
        );

        let mut o = 0usize;
        let mut cur_iface = None;
        while o + 2 <= total {
            let dl = raw[o] as usize;
            let dt = raw[o + 1];
            if dl < 2 || o + dl > total {
                break;
            }
            if dt == USB_DESC_IFACE && dl >= 9 {
                let iface_num = raw[o + 2];
                let iclass = raw[o + 5];
                let _isub = raw[o + 6];
                let iproto = raw[o + 7];
                if iclass == USB_CLASS_HID {
                    cur_iface = Some((iface_num, iproto));
                } else {
                    cur_iface = None;
                }
            }
            if dt == USB_DESC_EP && dl >= 7 {
                if let Some((iface, proto)) = cur_iface {
                    let addr = raw[o + 2];
                    let attr = raw[o + 3];
                    let mps = u16::from_le_bytes([raw[o + 4], raw[o + 5]]);
                    let interval = raw[o + 6];
                    if (addr & 0x80) != 0 && (attr & 3) == 3 {
                        // Interrupt IN
                        if let Err(_e) =
                            self.init_single_hid(slot, csz, port, iface, proto, addr, mps, interval)
                        {
                        }
                    }
                }
            }
            o += dl;
        }
        Ok(())
    }

    fn init_single_hid(
        &mut self,
        slot: u8,
        csz: usize,
        port: u8,
        iface: u8,
        proto: u8,
        ep_addr: u8,
        mps: u16,
        interval: u8,
    ) -> DeviceResult<()> {
        let mut real_proto = proto;
        if real_proto == 0 {
            // bInterfaceProtocol 0 = "None" (non-boot HID). In the QEMU target
            // this is the `usb-tablet`, which reports *absolute* coordinates
            // (buttons, X16, Y16, wheel). Routing it to HID_PROTO_MOUSE made the
            // boot-mouse parser read the high byte of the absolute X as a
            // relative dy (always ≥0), pinning the pointer to the top of the
            // screen. Treat protocol-less HID pointers as the absolute tablet.
            // Real boot mice/keyboards report protocol 2/1 and are unaffected.
            real_proto = HID_PROTO_TABLET;
        }

        let report_len = match real_proto {
            HID_PROTO_KEY => 8usize,
            HID_PROTO_MOUSE => 8usize, // Soportar ratones con más botones/ruedas (boot extendido)
            // Read a full 8-byte report: the QEMU tablet's report is
            // [buttons, X16, Y16, wheel] (6 bytes), but requesting only 6
            // overruns the interrupt TRB and stalls the endpoint after the
            // first report on devices whose wMaxPacketSize is 8 — the pointer
            // would initialise at the centre and then freeze. 8 fits both.
            HID_PROTO_TABLET => 8usize,
            _ => 8usize,
        };

        // Forzar protocolo de boot (si el dispositivo lo soporta).
        // Algunos dispositivos no responden a SET_PROTOCOL → timeout corto (200ms).
        let _ = self.ep0_control_out0_optional(
            slot,
            trb_setup(0x21, HID_REQ_SET_PROTOCOL, 0, iface as u16, 0, 0),
            true,
        );
        // SET_IDLE es opcional en USB HID spec: si el dispositivo no responde
        // (QEMU emula algunos dispositivos que ignoran SET_IDLE), continuar de todos modos.
        let _ = self.ep0_control_out0_optional(
            slot,
            trb_setup(0x21, HID_REQ_SET_IDLE, 0, iface as u16, 0, 0),
            true,
        );

        let epn = ep_addr & 0x0f;
        let dci = (epn * 2 + 1) as usize;
        if dci >= 32 {
            return Err(DeviceError::InvalidParam);
        }

        let cfg = DmaBuf::new(33 * csz, 64)?;
        // Input Control Context: add Slot (A0) and the new endpoint (A_dci)
        cfg.write_u32(4, 0x01 | (1u32 << dci));

        // Copy the current Device Slot Context (at device-context offset 0) into the Input
        // Slot Context (at input-context offset csz).  The xHCI spec requires software to
        // supply a complete, valid Slot Context whenever A0=1 in a Configure Endpoint
        // command – writing zeros would corrupt the USB device address and port fields.
        if let Some(dev) = self.dev_ctx.get(slot as usize).and_then(|o| o.as_ref()) {
            dev.flush(0, csz); // Invalidate cache lines so we read the fresh Device Context updated by the controller
            for i in 0..(csz / 4) {
                cfg.write_u32(csz + i * 4, dev.read_u32(i * 4));
            }
        }
        // Raise Context Entries to cover the new endpoint DCI.
        let slot_dw0 = cfg.read_u32(csz);
        let cur_entries = (slot_dw0 >> 27) & 0x1f;
        let new_entries = (dci as u32).max(cur_entries);
        cfg.write_u32(csz, (slot_dw0 & !(0x1f << 27)) | (new_entries << 27));

        let ep_off = csz + csz + (dci - 1) * csz;

        // Endpoint Context DW0: set Interval from the USB endpoint descriptor.
        // For HS/SS (speed >= 3): USB bInterval is the exponent N where the period is
        // 2^(N-1) × 125 µs, but xHCI Interval is M where the period is 2^M × 125 µs.
        // Therefore xhci_interval = bInterval - 1 (per xHCI spec §6.2.3.6).
        // For FS/LS: bInterval is in frames (1 ms each); the field must be at least 1.
        let speed = self.slot_speed[slot as usize];
        let xhci_interval = if speed >= 3 {
            interval.saturating_sub(1).min(15)
        } else {
            interval.min(15).max(1)
        };
        cfg.write_u32(ep_off, (xhci_interval as u32) << 24);

        // Endpoint Context DW1: Error Count field (bits 2:1) = 3 (value 3 << 1 = 0b110),
        // EP Type, Max Packet Size.  Error Count = 3 allows up to 3 retries after failure.
        let ep_ty = (3u32 << 1) | EP_TYPE_INT_IN | ((mps as u32) << 16);
        cfg.write_u32(ep_off + 4, ep_ty);
        let ir = XferRing::new(64)?;
        let irp = ir.ring_phys() | 1; // DCS = 1
        cfg.write_u64(ep_off + 8, irp);
        if csz >= 64 {
            cfg.write_u32(ep_off + 16, (report_len as u32).min(0xffff));
        }
        let ridx = Self::ri(slot, dci as u8);
        self.xfer_rings[ridx] = Some(ir);

        let p = self
            .cmd
            .push(trb_configure_endpoint(cfg.sub_phys(0), slot))?;
        cfg.flush(0, 33 * csz);
        self.mmio.ring_db(0, 0);
        self.wait_cmd_phys(p)?;

        // Allocate one report buffer per pre-queued TRB so the controller can
        // race ahead by HID_QUEUE_DEPTH transfers without overwriting a buffer
        // we haven't dispatched yet. See HID_QUEUE_DEPTH docs for the why.
        let mut bufs = Vec::with_capacity(HID_QUEUE_DEPTH);
        for _ in 0..HID_QUEUE_DEPTH {
            bufs.push(DmaBuf::new(report_len, 64)?);
        }
        {
            let ring = self
                .xfer_rings
                .get_mut(ridx)
                .and_then(|o| o.as_mut())
                .ok_or(DeviceError::NotSupported)?;
            for buf in &bufs {
                let norm = trb_normal(buf.sub_phys(0), report_len as u16, true);
                let _ = ring.push(norm)?;
            }
        }
        self.mmio.ring_db(slot, dci as u8);

        self.hids.push(HidDev {
            slot_id: slot,
            port_id: port,
            ep_dci: dci as u8,
            ring_idx: ridx,
            protocol: real_proto,
            report_len,
            bufs,
            dispatch_idx: 0,
            enqueue_idx: 0,
            last_mods: 0,
            last_keys: [0; 6],
            tab_x: 0,
            tab_y: 0,
            tab_init: false,
        });
        Ok(())
    }

    fn dispatch_hid(&mut self, idx: usize, lis: &EventListener<InputEvent>) {
        let h = match self.hids.get_mut(idx) {
            Some(h) => h,
            None => return,
        };
        // Read from the buffer at the dispatch head of the round-robin ring,
        // then advance — the next event will dispatch the next buffer.
        let dispatch_idx = h.dispatch_idx;
        let report_len = h.report_len;
        h.dispatch_idx = (dispatch_idx + 1) % HID_QUEUE_DEPTH;
        let buf_phys = h.bufs[dispatch_idx].phys;
        let v = phys_to_virt(buf_phys);
        let mut tmp = [0u8; 8];
        let n = report_len.min(tmp.len()).min(8);
        tmp[..n].fill(0);
        // Asegurar consistencia de datos en arquitecturas con caché no coherente o mapeos WB.
        // Se debe invalidar ANTES de copiar los datos para que la CPU lea de la RAM (DMA).
        #[cfg(target_arch = "x86_64")]
        {
            let mut addr = v;
            let end = v + report_len as usize;
            while addr < end {
                unsafe {
                    _mm_clflush(addr as *const u8);
                }
                addr += 64;
            }
            unsafe {
                _mm_lfence();
            }
        }

        unsafe {
            core::ptr::copy_nonoverlapping(v as *const u8, tmp.as_mut_ptr(), n);
        }

        match h.protocol {
            HID_PROTO_KEY if h.report_len >= 8 => {
                let mods = tmp[0];
                let keys = [tmp[2], tmp[3], tmp[4], tmp[5], tmp[6], tmp[7]];
                emit_keyboard_delta(lis, h.last_mods, mods, &h.last_keys, &keys);
                h.last_mods = mods;
                h.last_keys = keys;
            }
            HID_PROTO_MOUSE if h.report_len >= 3 => {
                let btn = tmp[0];
                let dx = tmp[1] as i8 as i32;
                let dy = tmp[2] as i8 as i32;
                for (mask, code) in [
                    (1u8, BTN_LEFT),
                    (2u8, BTN_RIGHT),
                    (4u8, BTN_MIDDLE),
                    (8u8, BTN_SIDE),
                    (16u8, BTN_EXTRA),
                ] {
                    let down = (btn & mask) != 0;
                    let was = (h.last_mods & mask) != 0;
                    if down != was {
                        lis.trigger(InputEvent {
                            event_type: InputEventType::Key,
                            code,
                            value: if down { 1 } else { 0 },
                        });
                    }
                }
                h.last_mods = btn;
                if dx != 0 {
                    lis.trigger(InputEvent {
                        event_type: InputEventType::RelAxis,
                        code: REL_X,
                        value: dx,
                    });
                }
                if dy != 0 {
                    lis.trigger(InputEvent {
                        event_type: InputEventType::RelAxis,
                        code: REL_Y,
                        // USB HID reports Y as down-positive, exactly the evdev
                        // REL_Y convention libinput expects — emit as-is. (The
                        // earlier `-dy` was copied from the PS/2 driver, where
                        // +Y means up; under libinput it inverted the axis.)
                        value: dy,
                    });
                }
                if h.report_len >= 4 {
                    let w = tmp[3] as i8 as i32;
                    if w != 0 {
                        lis.trigger(InputEvent {
                            event_type: InputEventType::RelAxis,
                            code: REL_WHEEL,
                            value: -w,
                        });
                    }
                }
                if h.report_len >= 5 {
                    let hw = tmp[4] as i8 as i32;
                    if hw != 0 {
                        lis.trigger(InputEvent {
                            event_type: InputEventType::RelAxis,
                            code: REL_HWHEEL,
                            value: hw,
                        });
                    }
                }
                lis.trigger(InputEvent {
                    event_type: InputEventType::Syn,
                    code: SYN_REPORT,
                    value: 0,
                });
            }
            HID_PROTO_TABLET if h.report_len >= 6 => {
                let btn = tmp[0];
                let ax = u16::from_le_bytes([tmp[1], tmp[2]]) as u32;
                let ay = u16::from_le_bytes([tmp[3], tmp[4]]) as u32;
                let sx = ((ax as u64 * self.fb_width as u64) / (TABLET_RANGE as u64 + 1))
                    .min(self.fb_width as u64 - 1) as i32;
                let sy = ((ay as u64 * self.fb_height as u64) / (TABLET_RANGE as u64 + 1))
                    .min(self.fb_height as u64 - 1) as i32;
                if !h.tab_init {
                    h.tab_init = true;
                    h.tab_x = sx as u16;
                    h.tab_y = sy as u16;
                } else {
                    let dx = (sx - h.tab_x as i32).clamp(-127, 127);
                    let dy = (sy - h.tab_y as i32).clamp(-127, 127);
                    h.tab_x = sx as u16;
                    h.tab_y = sy as u16;
                    for (mask, code) in [(1u8, BTN_LEFT), (2u8, BTN_RIGHT), (4u8, BTN_MIDDLE)] {
                        let down = (btn & mask) != 0;
                        let was = (h.last_mods & mask) != 0;
                        if down != was {
                            lis.trigger(InputEvent {
                                event_type: InputEventType::Key,
                                code,
                                value: if down { 1 } else { 0 },
                            });
                        }
                    }
                    h.last_mods = btn;
                    if dx != 0 {
                        lis.trigger(InputEvent {
                            event_type: InputEventType::RelAxis,
                            code: REL_X,
                            value: dx,
                        });
                    }
                    if dy != 0 {
                        lis.trigger(InputEvent {
                            event_type: InputEventType::RelAxis,
                            code: REL_Y,
                            // The tablet already reports screen-space Y (0 = top,
                            // growing downward), so `dy > 0` means the pointer
                            // moved down. Emit it as-is: the `/dev/input/mice`
                            // PS/2 path negates once and kdrive negates again
                            // (PS/2 treats +Y as up), so the two cancel and the
                            // pointer follows the motion. Negating here inverts
                            // the vertical axis under Xfbdev.
                            value: dy,
                        });
                    }
                    lis.trigger(InputEvent {
                        event_type: InputEventType::Syn,
                        code: SYN_REPORT,
                        value: 0,
                    });
                }
            }
            _ => {}
        }
    }

    /// One-shot diagnostic dump when the controller halts. HCHalted (USBSTS
    /// bit 0) is only the symptom; the *cause* is in HSE (host system / DMA
    /// error) or HCE (internal error), and in the physical addresses we hand
    /// the controller. On a >4 GiB-RAM machine those DMA structures can land
    /// above 4 GiB; if the controller is not 64-bit-capable (AC64=0) — or a
    /// pointer is truncated — that is an immediate HSE. This is exactly the
    /// kind of fault that never reproduces under QEMU (2 GiB, low addresses).
    fn dump_halt_diagnostics(&self) {
        let sts = self.mmio.read_op(4);
        let cmd = self.mmio.read_op(0);
        let hcc1 = self.mmio.read_cap(0x10);
        let ac64 = (hcc1 & 1) != 0;
        let csz = (hcc1 & (1 << 2)) != 0;

        // An all-ones USBSTS is not a real status (high bits are reserved-0):
        // the controller stopped answering MMIO (powered down / off the bus).
        // Decoding the "bits" would be meaningless noise, so say so plainly.
        if sts == u32::MAX {
            error!(
                "[xhci] HALT diag: USBSTS=0xffffffff -> controller not responding to MMIO (powered down D3 or removed); not a real halt"
            );
            return;
        }

        error!(
            "[xhci] HALT diag: USBSTS={:#010x} [HCH={} HSE={} EINT={} PCD={} SRE={} CNR={} HCE={}]",
            sts,
            sts & 1,
            (sts >> 2) & 1,
            (sts >> 3) & 1,
            (sts >> 4) & 1,
            (sts >> 10) & 1,
            (sts >> 11) & 1,
            (sts >> 12) & 1,
        );
        error!(
            "[xhci] HALT diag: USBCMD={:#010x} [RS={} INTE={} HSEE={}] AC64={} CSZ(64B ctx)={}",
            cmd,
            cmd & 1,
            (cmd >> 2) & 1,
            (cmd >> 3) & 1,
            ac64,
            csz
        );

        const FOUR_GIB: u64 = 1 << 32;
        let dcbaa = self.dcbaa.phys as u64;
        let erst = self.ev.erst_phys();
        let evseg = self.ev.seg.phys as u64;
        let crcr = self.cmd.crcr() as u64;
        let scratch = self
            .scratch_tbl
            .as_ref()
            .map(|t| t.phys as u64)
            .unwrap_or(0);
        let any_high = [dcbaa, erst, evseg, crcr, scratch]
            .iter()
            .any(|&a| a >= FOUR_GIB);
        error!(
            "[xhci] HALT diag: DCBAA={:#x} ERST={:#x} EVSEG={:#x} CRCR={:#x} SCRATCH={:#x} -> any>4GiB={}",
            dcbaa, erst, evseg, crcr, scratch, any_high
        );
        if any_high && !ac64 {
            error!("[xhci] HALT diag: *** AC64=0 (32-bit controller) with DMA >4GiB -> very likely the HSE/halt cause ***");
        }
    }

    fn process_irq_events(&mut self, lis: Option<&EventListener<InputEvent>>) {
        for _ in 0..512 {
            if self.pop_ev(lis).is_none() {
                break;
            }
        }
        // En modo poll/IRQ, procesar los cambios de puerto diferidos ahora que
        // no estamos en medio de enumeración.
        self.drain_pending_port_changes();
        // Recover any HID endpoint that stalled during the drain above.
        self.drain_pending_ep_resets();
    }

    /// Reset and re-arm every HID interrupt endpoint that errored during the
    /// last event drain (see `pending_ep_resets`). Runs outside `pop_ev`, so the
    /// Reset Endpoint / Set TR Dequeue commands (which spin on the event ring)
    /// don't recurse into the drain loop.
    fn drain_pending_ep_resets(&mut self) {
        let resets: Vec<(u8, u8)> = self.pending_ep_resets.drain(..).collect();
        for (slot, dci) in resets {
            // Reset the halted endpoint and point its TR dequeue past the failed
            // TRB (already skipped via advance_dequeue), then re-arm one TRB and
            // ring the doorbell so interrupt transfers resume.
            if self.reset_endpoint_and_dequeue(slot, dci).is_err() {
                continue;
            }
            if let Some(idx) = self
                .hids
                .iter()
                .position(|h| h.slot_id == slot && h.ep_dci == dci)
            {
                let (ridx, blen, buf_phys) = {
                    let h = &self.hids[idx];
                    (
                        h.ring_idx,
                        (h.report_len.min(64)) as u16,
                        h.bufs[h.enqueue_idx].sub_phys(0),
                    )
                };
                self.resubmit_hid_normal_trb(ridx, buf_phys, blen, slot, dci);
                if let Some(h) = self.hids.get_mut(idx) {
                    h.enqueue_idx = (h.enqueue_idx + 1) % HID_QUEUE_DEPTH;
                }
                warn!(
                    "[xhci] recovered HID endpoint slot={} dci={} after a transfer error",
                    slot, dci
                );
            }
        }
    }

    /// Best-effort recovery from an HCHalted controller without a full HCRST
    /// (which would tear down enumerated devices). Clears the sticky USBSTS
    /// error bits and re-asserts Run/Stop. Returns `true` if the controller
    /// observably un-halted (USBSTS.HCH cleared); the caller can then resume
    /// normal polling. Rate-limited by a backoff window so we don't pound the
    /// MMIO when the halt is genuine.
    fn try_soft_recover(&mut self) -> bool {
        let now = timer_now_us();
        if self.halt_last_attempt_us != 0
            && now.wrapping_sub(self.halt_last_attempt_us) < HALT_RECOVERY_BACKOFF_US
        {
            return false;
        }
        self.halt_last_attempt_us = now;
        self.halt_attempts = self.halt_attempts.saturating_add(1);

        let sts = self.mmio.read_op(4);
        // PCI / D3: not recoverable from this driver.
        if sts == u32::MAX {
            return false;
        }
        // Not actually halted anymore? Treat as recovered.
        if sts & 1 == 0 {
            self.halt_attempts = 0;
            return true;
        }
        // Clear sticky / W1C bits: HSE (bit 2), EINT (3), PCD (4), SRE (10).
        self.mmio
            .write_op(4, (1 << 2) | (1 << 3) | (1 << 4) | (1 << 10));
        // Re-assert RS=1. If INTE was on before, preserve it; if MSI is wired,
        // make sure it stays enabled.
        let mut cmd = self.mmio.read_op(0);
        cmd |= 1;
        if self.msi_vector > 0 {
            cmd |= 1 << 2;
        }
        self.mmio.write_op(0, cmd);

        // Brief wait for HCH to drop. A genuinely dead controller will time
        // out here; a transient bus error usually clears within a few µs.
        let start = timer_now_us();
        let mut spins = 0u64;
        while !xhci_wait_expired(start, HALT_RECOVERY_WAIT_US, spins) {
            if self.mmio.read_op(4) & 1 == 0 {
                warn!(
                    "[xhci] HCHalted recovered after soft restart (attempt {})",
                    self.halt_attempts
                );
                self.halt_attempts = 0;
                return true;
            }
            spins = spins.saturating_add(1);
            spin_loop();
        }
        false
    }

    fn handle_port_status_change(&mut self, port_id: u8) -> DeviceResult<()> {
        let off = 0x400 + (port_id as usize - 1) * 0x10;
        let sc = self.mmio.read_op(off);
        if (sc & (1 << 17)) != 0 {
            let ccs = (sc & 1) != 0;
            info!("[xhci] puerto {}: CSC, CCS={}", port_id, ccs);
            if ccs {
                if let Err(e) = self.try_port_hid(port_id) {
                    warn!("[xhci] fallo al enumerar puerto {}: {:?}", port_id, e);
                }
            } else {
                self.cleanup_port(port_id)?;
            }
        }
        // Limpiar RW1C sin apagar el puerto (PP) ni tocar bits RW como PED.
        self.mmio
            .write_op(off, (sc & !PORTSC_RW1C_AND_RO_MASK) | PORTSC_CHANGE_BITS);
        Ok(())
    }

    fn cleanup_port(&mut self, port_id: u8) -> DeviceResult<()> {
        let mut found_slot = None;
        for s in 1..=self.max_slots {
            if self.slot_port[s as usize] == port_id {
                found_slot = Some(s);
                break;
            }
        }
        if let Some(slot) = found_slot {
            info!(
                "[xhci] desconexión en puerto {}, liberando slot {}",
                port_id, slot
            );
            let _ = self.exec_cmd(trb_disable_slot(slot));
            self.dev_ctx[slot as usize] = None;
            self.slot_port[slot as usize] = 0;
            self.slot_speed[slot as usize] = 0;
            for ep in 1..32 {
                let ri = Self::ri(slot, ep);
                if ri < self.xfer_rings.len() {
                    self.xfer_rings[ri] = None;
                }
            }
            self.hids.retain(|h| h.slot_id != slot);
        }
        Ok(())
    }
}

fn hid_usage_to_linux(u: u8) -> Option<u16> {
    Some(match u {
        0x04 => KEY_A,
        0x05 => KEY_B,
        0x06 => KEY_C,
        0x07 => KEY_D,
        0x08 => KEY_E,
        0x09 => KEY_F,
        0x0a => KEY_G,
        0x0b => KEY_H,
        0x0c => KEY_I,
        0x0d => KEY_J,
        0x0e => KEY_K,
        0x0f => KEY_L,
        0x10 => KEY_M,
        0x11 => KEY_N,
        0x12 => KEY_O,
        0x13 => KEY_P,
        0x14 => KEY_Q,
        0x15 => KEY_R,
        0x16 => KEY_S,
        0x17 => KEY_T,
        0x18 => KEY_U,
        0x19 => KEY_V,
        0x1a => KEY_W,
        0x1b => KEY_X,
        0x1c => KEY_Y,
        0x1d => KEY_Z,
        0x1e => KEY_1,
        0x1f => KEY_2,
        0x20 => KEY_3,
        0x21 => KEY_4,
        0x22 => KEY_5,
        0x23 => KEY_6,
        0x24 => KEY_7,
        0x25 => KEY_8,
        0x26 => KEY_9,
        0x27 => KEY_0,
        0x28 => KEY_ENTER,
        0x29 => KEY_ESC,
        0x2a => KEY_BACKSPACE,
        0x2b => KEY_TAB,
        0x2c => KEY_SPACE,
        0x2d => KEY_MINUS,
        0x2e => KEY_EQUAL,
        0x2f => KEY_LEFTBRACE,
        0x30 => KEY_RIGHTBRACE,
        0x31 => KEY_BACKSLASH,
        0x32 => KEY_102ND,
        0x64 => KEY_102ND,
        0x33 => KEY_SEMICOLON,
        0x34 => KEY_APOSTROPHE,
        0x35 => KEY_GRAVE,
        0x36 => KEY_COMMA,
        0x37 => KEY_DOT,
        0x38 => KEY_SLASH,
        0x39 => KEY_CAPSLOCK,
        0x3a => KEY_F1,
        0x3b => KEY_F2,
        0x3c => KEY_F3,
        0x3d => KEY_F4,
        0x3e => KEY_F5,
        0x3f => KEY_F6,
        0x40 => KEY_F7,
        0x41 => KEY_F8,
        0x42 => KEY_F9,
        0x43 => KEY_F10,
        0x44 => KEY_F11,
        0x45 => KEY_F12,
        0x46 => KEY_SYSRQ,
        0x47 => KEY_SCROLLLOCK,
        0x48 => KEY_PAUSE,
        0x49 => KEY_INSERT,
        0x4a => KEY_HOME,
        0x4b => KEY_PAGEUP,
        0x4c => KEY_DELETE,
        0x4d => KEY_END,
        0x4e => KEY_PAGEDOWN,
        0x4f => KEY_RIGHT,
        0x50 => KEY_LEFT,
        0x51 => KEY_DOWN,
        0x52 => KEY_UP,
        0x53 => KEY_NUMLOCK,
        0x54 => KEY_KPSLASH,
        0x55 => KEY_KPASTERISK,
        0x56 => KEY_KPMINUS,
        0x57 => KEY_KPPLUS,
        0x58 => KEY_KPENTER,
        0x59 => KEY_KP1,
        0x5a => KEY_KP2,
        0x5b => KEY_KP3,
        0x5c => KEY_KP4,
        0x5d => KEY_KP5,
        0x5e => KEY_KP6,
        0x5f => KEY_KP7,
        0x60 => KEY_KP8,
        0x61 => KEY_KP9,
        0x62 => KEY_KP0,
        0x63 => KEY_KPDOT,
        0xe0 => KEY_LEFTCTRL,
        0xe1 => KEY_LEFTSHIFT,
        0xe2 => KEY_LEFTALT,
        0xe3 => KEY_LEFTMETA,
        0xe4 => KEY_RIGHTCTRL,
        0xe5 => KEY_RIGHTSHIFT,
        0xe6 => KEY_RIGHTALT,
        0xe7 => KEY_RIGHTMETA,
        _ => return None,
    })
}

fn emit_keyboard_delta(
    lis: &EventListener<InputEvent>,
    prev_m: u8,
    new_m: u8,
    prev_k: &[u8; 6],
    new_k: &[u8; 6],
) {
    for bit in 0u8..8u8 {
        let m = 1u8 << bit;
        let was = (prev_m & m) != 0;
        let now = (new_m & m) != 0;
        if was == now {
            continue;
        }
        let code = match bit {
            0 => KEY_LEFTCTRL,
            1 => KEY_LEFTSHIFT,
            2 => KEY_LEFTALT,
            3 => KEY_LEFTMETA,
            4 => KEY_RIGHTCTRL,
            5 => KEY_RIGHTSHIFT,
            6 => KEY_RIGHTALT,
            7 => KEY_RIGHTMETA,
            _ => continue,
        };
        lis.trigger(InputEvent {
            event_type: InputEventType::Key,
            code,
            value: if now { 1 } else { 0 },
        });
    }
    'p: for &u in new_k {
        if u == 0 {
            continue;
        }
        for &v in prev_k {
            if v == u {
                continue 'p;
            }
        }
        if let Some(c) = hid_usage_to_linux(u) {
            lis.trigger(InputEvent {
                event_type: InputEventType::Key,
                code: c,
                value: 1,
            });
        }
    }
    'r: for &u in prev_k {
        if u == 0 {
            continue;
        }
        for &v in new_k {
            if v == u {
                continue 'r;
            }
        }
        if let Some(c) = hid_usage_to_linux(u) {
            lis.trigger(InputEvent {
                event_type: InputEventType::Key,
                code: c,
                value: 0,
            });
        }
    }
    lis.trigger(InputEvent {
        event_type: InputEventType::Syn,
        code: SYN_REPORT,
        value: 0,
    });
}

pub struct XhciUsbHid {
    listener: EventListener<InputEvent>,
    inner: Mutex<Option<XhciInner>>,
    pub msi_vector: usize,
}

/// Instancia global para drenar el event ring desde el timer (QEMU / IRQ perdidos).
static POLL_INSTANCE: Mutex<Option<Arc<XhciUsbHid>>> = Mutex::new(None);

pub fn set_poll_instance(dev: Option<Arc<XhciUsbHid>>) {
    *POLL_INSTANCE.lock() = dev;
}

/// Respaldo periódico: drena transferencias HID sin depender de MSI (alineado al driver de referencia).
pub fn poll() {
    // Fast path: a controller we have given up on (latched halt) has nothing
    // to poll, so bail before any lock or MMIO. Unlike the old code we only
    // reach this latch after MAX_HALT_RECOVERY_ATTEMPTS soft recoveries have
    // all failed — so a transient HSE no longer kills input forever on real
    // hardware.
    if XHCI_HALTED.load(Ordering::Relaxed) {
        return;
    }
    let inst = POLL_INSTANCE.lock();
    if let Some(d) = &*inst {
        let mut g = d.inner.lock();
        if let Some(xi) = &mut *g {
            if xi.boot_enum_pending {
                xi.boot_enum_pending = false;
                info!("[xhci] deferred boot enumeration starting");
                xi.enumerate_root_hid();
            }
            xi.mmio.ack_host_interrupt();
            let sts = xi.mmio.read_op(4);
            // `0xffffffff` is not a valid USBSTS (its high bits are reserved-0):
            // an all-ones read means the controller stopped responding to MMIO,
            // i.e. it powered down (D3) or fell off the bus — typical of an
            // unused GPU USB-C / VirtualLink xHCI with nothing plugged in. That
            // is not recoverable from this driver: latch hard.
            if sts == u32::MAX {
                if !XHCI_HALTED.swap(true, Ordering::Relaxed) {
                    warn!("[xhci] USBSTS=0xffffffff: el controlador no responde (apagado D3 o ausente, p.ej. un puerto USB-C/VirtualLink de GPU vacío); se detiene el sondeo");
                    xi.dump_halt_diagnostics();
                }
                return;
            }
            if sts & 1 != 0 {
                // Genuine HCHalted. Don't give up: try a soft recovery (clear
                // sticky USBSTS errors + re-assert RS) up to a few times before
                // permanently latching. Most real-HW halts seen so far are
                // transient HSE bursts that clear with a single restart, but
                // latching forever would make input dead until reboot — exactly
                // the "TinyX kills the keyboard after a few seconds" symptom.
                let attempts = xi.halt_attempts;
                if attempts < MAX_HALT_RECOVERY_ATTEMPTS {
                    if attempts == 0 {
                        warn!(
                            "[xhci] USBSTS=HCHalted; intentando recuperación suave (attempt {}/{})",
                            attempts + 1,
                            MAX_HALT_RECOVERY_ATTEMPTS
                        );
                        xi.dump_halt_diagnostics();
                    }
                    if !xi.try_soft_recover() {
                        return;
                    }
                    // Fall through to normal event drain.
                } else {
                    if !XHCI_HALTED.swap(true, Ordering::Relaxed) {
                        warn!(
                            "[xhci] HCHalted no se recupera tras {} intentos; se detiene el sondeo",
                            MAX_HALT_RECOVERY_ATTEMPTS
                        );
                    }
                    return;
                }
            } else if xi.halt_attempts != 0 {
                // Controller is healthy again — clear the soft-recovery
                // budget so the next halt (if any) gets its own fresh shot.
                xi.halt_attempts = 0;
            }
            xi.process_irq_events(Some(&d.listener));
        }
    }
}

impl XhciUsbHid {
    pub fn probe(
        dev: &PCIDevice,
        mmio_vaddr: usize,
        bar_size: usize,
        msi_vector: usize,
    ) -> DeviceResult<Arc<Self>> {
        let _ = dev;
        let mmio = XhciMmio::from_virt(mmio_vaddr, bar_size)?;
        let hcsp = mmio.read_cap(4);
        let max_slots = (hcsp & 0xff) as u8;
        if max_slots == 0 {
            return Err(DeviceError::InvalidParam);
        }
        let max_ports = ((hcsp >> 24) & 0xff) as u8;
        let mut inner = XhciInner::new(mmio, max_slots, max_ports, msi_vector)?;
        inner.reset_and_run()?;
        // HID enumeration is deferred to the first poll() so PCI probe stays fast at 80%.
        let arc = Arc::new(Self {
            listener: EventListener::new(),
            inner: Mutex::new(Some(inner)),
            msi_vector,
        });
        set_poll_instance(Some(arc.clone()));
        Ok(arc)
    }
}

impl_event_scheme!(XhciUsbHid, InputEvent);

impl Scheme for XhciUsbHid {
    fn name(&self) -> &str {
        "xhci-usb-hid"
    }

    fn handle_irq(&self, vector: usize) {
        if vector != self.msi_vector {
            return;
        }
        let mut g = self.inner.lock();
        if let Some(ref mut xi) = *g {
            xi.mmio.ack_host_interrupt();
            xi.process_irq_events(Some(&self.listener));
        }
    }
}

impl InputScheme for XhciUsbHid {
    fn capability(&self, cap_type: CapabilityType) -> InputCapability {
        let mut cap = InputCapability::empty();
        match cap_type {
            CapabilityType::Event => cap.set_all(&[EV_SYN, EV_KEY, EV_REL]),
            CapabilityType::Key => {
                // Advertise the full keyboard keycode block plus the mouse
                // buttons this HID scheme can emit. libinput builds the
                // device's key set from EVIOCGBIT(EV_KEY) and DROPS any key
                // event whose code is not in this bitmap. The previous list
                // named only a sparse sample of keys (KEY_A and KEY_Z as if
                // they were range endpoints, KEY_0/KEY_9, KEY_F1/KEY_F12) and
                // NONE of the keys in between — so the compositor only ever
                // received the handful of explicitly-listed keys (notably just
                // 'a' among the letters). `hid_usage_to_linux` maps HID usages
                // across the whole 1..=248 keyboard range, so advertise all of
                // it, exactly as a real keyboard does.
                for code in KEY_ESC..=KEY_MICMUTE {
                    cap.set(code);
                }
                cap.set_all(&[BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, BTN_SIDE, BTN_EXTRA]);
            }
            CapabilityType::RelAxis => cap.set_all(&[REL_X, REL_Y, REL_WHEEL, REL_HWHEEL]),
            _ => {}
        }
        cap
    }
}

pub struct XhciDriverPci;

impl PciDriver for XhciDriverPci {
    fn name(&self) -> &str {
        "xhci"
    }

    fn matched(&self, vendor_id: u16, _device_id: u16) -> bool {
        // We match by class/subclass/prog_if in matched_dev instead.
        // But for simplicity, we can just return false here and use a custom logic in pci_drivers.
        // Actually, PciDriver trait should be flexible enough.
        // I'll add a matched_dev method to PciDriver if needed, but for now I'll just match common xHCI IDs or all USB controllers.
        vendor_id != 0xffff // temporary: we'll check class in init or matched
    }

    fn matched_dev(&self, dev: &PCIDevice) -> bool {
        dev.id.class == 0x0c && dev.id.subclass == 0x03
    }

    fn init(
        &self,
        dev: &PCIDevice,
        mapper: &Option<Arc<dyn IoMapper>>,
        irq: Option<usize>,
    ) -> DeviceResult<Device> {
        let (addr, len) = if let Some(BAR::Memory(ba, bl, _, _)) = dev.bars[0] {
            (ba, bl as u64)
        } else {
            return Err(DeviceError::NotSupported);
        };

        if addr == 0 {
            return Err(DeviceError::NotSupported);
        }

        let base_addr = (addr as usize) & !0xfff;
        let offset = (addr as usize) & 0xfff;
        let map_len =
            ((len.min(usize::MAX as u64) as usize + offset + 0xfff) & !0xfff).max(128 * 1024);

        if let Some(m) = mapper {
            m.query_or_map(base_addr, map_len);
        }

        let vaddr = crate::bus::phys_to_virt(addr as usize);

        let vector = irq.map(|idx| idx + 32).unwrap_or(NO_MSI_VECTOR);

        // Handle xHCI
        if dev.id.prog_if == 0x30 {
            let input = XhciUsbHid::probe(dev, vaddr, map_len, vector)?;
            if vector != NO_MSI_VECTOR {
                pci_note_pending_msi(vector, input.clone());
            }
            Ok(Device::Input(input))
        } else {
            // Legacy USB
            #[cfg(feature = "legacy-usb-hid")]
            {
                let kind = match dev.id.prog_if {
                    0x20 => LegacyUsbKind::Ehci,
                    0x10 => LegacyUsbKind::Ohci,
                    0x00 => LegacyUsbKind::Uhci,
                    _ => return Err(DeviceError::NotSupported),
                };
                let input = LegacyUsbHid::probe(kind, dev, vaddr, map_len, vector)?;
                Ok(Device::Input(input))
            }
            #[cfg(not(feature = "legacy-usb-hid"))]
            Err(DeviceError::NotSupported)
        }
    }
}
