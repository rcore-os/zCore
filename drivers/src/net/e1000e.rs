//! Intel e1000e NIC driver — simplified for I219 (PCH-SPT / 82574L / QEMU e1000)
//!
//! Based on the Onyx (heatd/Onyx) and LittleKernel e1000 reference drivers.
//! No AMT open sequence, no BM WUC filter management, no complex MDIO autoneg.
//! Reset → read MAC → init rings → enable RX/TX → done.

#![allow(unused_imports, dead_code)]

const E1000E_DRIVER_TAG: &str = "e1000e-lk-rx2";
const E1000E_WATCHDOG_PERIOD_US: u64 = 2_000_000;
const E1000E_WATCHDOG_FAST_US: u64 = 50_000;
const E1000E_WATCHDOG_LOG_US: u64 = 5_000_000;
const E1000E_LOG_VERBOSE: bool = false;
const E1000E_ITR_LOW_LATENCY: u32 = 98;
const E1000E_ITR_BALANCED: u32 = 195;
const E1000E_ITR_THROUGHPUT: u32 = 512;
const E1000E_ITR_TUNE_PERIOD_US: u64 = 250_000;
/// If `poll_pending` (see [`E1000eInterface`]) has stayed true this long, the
/// IRQ bottom-half that owns clearing it was evicted from the shared
/// deferred-job queue and will never run — self-heal instead of staying
/// interrupt-deaf forever. Deferred jobs normally drain within a scheduler
/// tick or two, so this is comfortably above any legitimate latency.
const POLL_PENDING_STUCK_US: u64 = 500_000;

macro_rules! e1000e_vlog {
    ($($t:tt)*) => {
        if E1000E_LOG_VERBOSE { crate::klog_info!($($t)*); }
    };
}

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{compiler_fence, fence, AtomicBool, AtomicU64, Ordering};

use smoltcp::iface::*;
use smoltcp::phy::{self, Checksum, DeviceCapabilities};
use smoltcp::time::Instant;
use smoltcp::wire::*;
use smoltcp::Result as SmolResult;

use crate::builder::IoMapper;
use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
use crate::bus::pci_drivers::PciDriver;
use crate::net::get_sockets;
use crate::scheme::{NetScheme, NetStats, RouteInfo, Scheme, SchemeUpcast};
use crate::utils::dma::DmaRegion;
use crate::utils::dma_sync::{dma_sync_region, dma_sync_rx_desc_span, DmaSyncDir};
use crate::{Device, DeviceError, DeviceResult};
use lock::Mutex;
use pci::{Location, PCIDevice, BAR};

use super::timer_now_as_micros;

// ---------------------------------------------------------------------------
// Register offsets (byte address / 4 → u32 index into MMIO array)
// ---------------------------------------------------------------------------
const E1000E_CTRL: usize = 0x0000 / 4;
const E1000E_STATUS: usize = 0x0008 / 4;
const E1000E_EECD: usize = 0x0010 / 4;
const E1000E_EERD: usize = 0x0014 / 4;
const E1000E_CTRL_EXT: usize = 0x0018 / 4;
const E1000E_MDIC: usize = 0x0020 / 4;
const E1000E_EXTCNF_CTRL: usize = 0x0F00 / 4;
const E1000E_PHY_CTRL: usize = 0x00F10 / 4;
// Real offsets per Linux e1000e's regs.h — 0x01014/0x01018 (this file's
// previous values) land in reserved MMIO next to PBA, not on FEXTNVM6/7, so
// the ULP/SPT workarounds that RMW them were silently no-ops.
const E1000E_FEXTNVM6: usize = 0x00010 / 4;
const E1000E_FEXTNVM7: usize = 0x000E4 / 4;
const E1000E_PBA: usize = 0x01000 / 4;
const E1000E_ICR: usize = 0x00C0 / 4;
const E1000E_ITR: usize = 0x00C4 / 4;
const E1000E_IMS: usize = 0x00D0 / 4;
const E1000E_IAM: usize = 0x00E0 / 4;
const E1000E_IMC: usize = 0x00D8 / 4;
const E1000E_RCTL: usize = 0x0100 / 4;
const E1000E_TCTL: usize = 0x0400 / 4;
const E1000E_TIPG: usize = 0x0410 / 4;
const E1000E_RDBAL: usize = 0x2800 / 4;
const E1000E_RDBAH: usize = 0x2804 / 4;
const E1000E_RDLEN: usize = 0x2808 / 4;
const E1000E_RDH: usize = 0x2810 / 4;
const E1000E_RDT: usize = 0x2818 / 4;
const E1000E_RDTR: usize = 0x2820 / 4;
const E1000E_RXDCTL: usize = 0x02828 / 4;
const E1000E_RADV: usize = 0x282C / 4;
const E1000E_SRRCTL: usize = 0x0280C / 4;
const E1000E_TDBAL: usize = 0x3800 / 4;
const E1000E_TDBAH: usize = 0x3804 / 4;
const E1000E_TDLEN: usize = 0x3808 / 4;
const E1000E_TDH: usize = 0x3810 / 4;
const E1000E_TDT: usize = 0x3818 / 4;
const E1000E_TXDCTL: usize = 0x03828 / 4;
const E1000E_TXDCTL1: usize = E1000E_TXDCTL + (0x100 / 4);
const E1000E_TIDV: usize = 0x03820 / 4;
const E1000E_TADV: usize = 0x0382C / 4;
const E1000E_TARC0: usize = 0x03840 / 4;
const E1000E_RAL0: usize = 0x5400 / 4;
const E1000E_RAH0: usize = 0x5404 / 4;
const E1000E_MTA_BASE: usize = 0x5200 / 4;
const E1000E_RXCSUM: usize = 0x5000 / 4;
const E1000E_RFCTL: usize = 0x5008 / 4;
const E1000E_MRQC: usize = 0x5818 / 4;
const E1000E_VET: usize = 0x0038 / 4;
const E1000E_GPRC: usize = 0x04074 / 4;
const E1000E_GPTC: usize = 0x04080 / 4;
const E1000E_GORCL: usize = 0x04088 / 4;
const E1000E_GORCH: usize = 0x0408C / 4;
const E1000E_MPC: usize = 0x04010 / 4;
const E1000E_WUC: usize = 0x05800 / 4;
const E1000E_WUFC: usize = 0x05808 / 4;
const E1000E_WUS: usize = 0x05810 / 4;
const E1000E_MANC: usize = 0x05820 / 4;
const E1000E_FWSM: usize = 0x05B54 / 4;
const E1000E_H2ME: usize = 0x05B50 / 4;
const E1000E_IOSFPC: usize = 0x00F28 / 4;
const E1000E_VFTA_BASE: usize = 0x5600 / 4;

// CTRL register bits
const CTRL_FD: u32 = 1 << 0;
const CTRL_ASDE: u32 = 1 << 5;
const CTRL_SLU: u32 = 1 << 6;
// Per the Intel datasheet / Linux e1000_hw.h, FRCSPD is bit 11 (0x800) and
// FRCDPX is bit 12 (0x1000) — these were previously swapped. Harmless while
// both are only ever set/cleared together (as a pair), but wrong for any
// code that toggles one alone.
const CTRL_FRCSPD: u32 = 1 << 11;
const CTRL_FRCDPX: u32 = 1 << 12;
const CTRL_RST: u32 = 1 << 26;
const CTRL_PHY_RST: u32 = 1 << 31;

// STATUS register bits
const STATUS_LU: u32 = 1 << 1;
const STATUS_FD: u32 = 1 << 0;

// CTRL_EXT bits
const CTRL_EXT_RO_DIS: u32 = 1 << 17; // PCIe Relaxed Ordering Disable
const CTRL_EXT_DRV_LOAD: u32 = 1 << 28; // Driver loaded (release ME)
const CTRL_EXT_IAME: u32 = 1 << 27;

// PHY_CTRL (MAC-side register 0xF10) — LPLU bits for I219
const PHY_CTRL_D0A_LPLU: u32 = 1 << 1;
const PHY_CTRL_NOND0A_LPLU: u32 = 1 << 2;
const PHY_CTRL_NOND0A_GBE_DISABLE: u32 = 1 << 3;
const PHY_CTRL_GBE_DISABLE: u32 = 1 << 6;

// MDIC register bits
const MDIC_REG_SHIFT: u32 = 16;
const MDIC_PHYADD_SHIFT: u32 = 21;
const MDIC_OP_WRITE: u32 = 1 << 26;
const MDIC_OP_READ: u32 = 2 << 26;
const MDIC_READY: u32 = 1 << 28;
const MDIC_ERROR: u32 = 1 << 30;
const MDIC_POLL_TRIES: u32 = 2000;

// PHY register 0 (BMCR)
const BMCR_RESET: u16 = 0x8000;

// EERD
const EERD_START: u32 = 1 << 0;
const EERD_DONE_BIT4: u32 = 1 << 4;
const EERD_DONE_BIT1: u32 = 1 << 1;
const EERD_DATA_SHIFT: u32 = 16;

// ICR / IMS bits (LK e1000_hw.h)
const ICR_TXDW: u32 = 1 << 0;
const ICR_LSC: u32 = 1 << 2;
const ICR_RXDMT0: u32 = 1 << 4;
const ICR_RXO: u32 = 1 << 6;
const ICR_RXT0: u32 = 1 << 7;
const ICR_RXTO: u32 = ICR_RXT0;
const ICR_RX_WORK: u32 = ICR_RXTO | ICR_RXO | ICR_RXDMT0 | ICR_RXT0;
const ICR_RX_ANY: u32 = ICR_RX_WORK;
const IMS_REARM: u32 = ICR_TXDW | ICR_LSC | ICR_RX_WORK | (1 << 8);

// RCTL bits
const RCTL_EN: u32 = 1 << 1;
const RCTL_SBP: u32 = 1 << 2;
const RCTL_UPE: u32 = 1 << 3;
const RCTL_MPE: u32 = 1 << 4;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_SECRC: u32 = 1 << 26;

// TCTL bits
const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;
const TCTL_RTLC: u32 = 1 << 24;
const TCTL_CT_SHIFT: u32 = 4;
const TCTL_CT_LINUX: u32 = 15 << TCTL_CT_SHIFT;
const TCTL_COLD_LINUX: u32 = 63 << 12;

// TXDCTL / RXDCTL
const TXDCTL_QUEUE_ENABLE: u32 = 1 << 25;
const RXDCTL_QUEUE_ENABLE: u32 = 1 << 25;
const TXDCTL_FULL_TX_DESC_WB: u32 = 0x0101_0000;
const TXDCTL_DMA_BURST: u32 = (1 << 22) | (1 << 8) | 1; // wthresh=1, pthresh=1, hthresh=1

// RFCTL bits
const RFCTL_EXTEN: u32 = 1 << 15;
const RFCTL_NFSW_DIS: u32 = 1 << 6;
const RFCTL_NFSR_DIS: u32 = 1 << 7;

// MANC
const MANC_EN_MNG2HOST: u32 = 1 << 21;

// TARC0
const TARC0_SPEED_MODE: u32 = 1 << 21;

// ULP (Ultra Low Power) disable — i219/PCH-SPT only. On real hardware the ME
// firmware often leaves the PHY in ULP, so STATUS.LU never asserts. QEMU has no
// ME, which is why this is only needed on real hardware.
// FWSM.FW_VALID is bit 15 (0x8000); there used to be a second, wrongly-valued
// `FWSM_FW_VALID = 1 << 14` constant here that nothing referenced — removed
// rather than fixed in place, since a dead duplicate is a landmine for the
// next person who reaches for the "obviously right" name instead of this one.
const ICH_FWSM_FW_VALID: u32 = 0x0000_8000;
const FWSM_ULP_CFG_DONE: u32 = 0x0000_0400;
const H2ME_ULP: u32 = 0x0000_0800;
const H2ME_ENFORCE_SETTINGS: u32 = 0x0000_1000;
const FEXTNVM7_DISABLE_SMB_PERST: u32 = 0x0000_0020;
const CTRL_EXT_FORCE_SMBUS: u32 = 0x0000_0800; // CTRL_EXT bit 11

// SW/FW semaphore (EXTCNF_CTRL)
const EXTCNF_CTRL_SWFLAG: u32 = 0x0000_0020; // bit 5

// HV PHY paged-register access: value at page-select reg = page << PHY_PAGE_SHIFT,
// then access (reg & MAX_PHY_REG_ADDRESS) via MDIC.
const PHY_PAGE_SHIFT: u32 = 5;
const PHY_PAGE_SELECT_REG: u32 = 0x1F; // IGP01E1000_PHY_PAGE_SELECT
const MAX_PHY_REG_ADDRESS: u32 = 0x1F;
const MAX_PHY_MULTI_PAGE_REG: u32 = 0x0F;
const HV_PHY_ADDR: u8 = 1; // pages >= 768 live at PHY addr 1

// CV_SMB_CTRL = PHY_REG(769, 23)
const CV_SMB_CTRL_PAGE: u32 = 769;
const CV_SMB_CTRL_REG: u32 = 23;
const CV_SMB_CTRL_FORCE_SMBUS: u16 = 0x0001;
// HV_PM_CTRL = PHY_REG(770, 17)
const HV_PM_CTRL_PAGE: u32 = 770;
const HV_PM_CTRL_REG: u32 = 17;
const HV_PM_CTRL_K1_ENABLE: u16 = 0x4000;
// I218_ULP_CONFIG1 = PHY_REG(779, 16)
const ULP_CONFIG1_PAGE: u32 = 779;
const ULP_CONFIG1_REG: u32 = 16;
const ULP_CONFIG1_START: u16 = 0x0001;
const ULP_CONFIG1_IND: u16 = 0x0004;
const ULP_CONFIG1_STICKY_ULP: u16 = 0x0010;
const ULP_CONFIG1_INBAND_EXIT: u16 = 0x0020;
const ULP_CONFIG1_WOL_HOST: u16 = 0x0040;
const ULP_CONFIG1_RESET_TO_SMBUS: u16 = 0x0100;
const ULP_CONFIG1_DISABLE_SMB_PERST: u16 = 0x1000;

// GIO master disable (quiesce DMA before CTRL_RST)
const CTRL_GIO_MASTER_DISABLE: u32 = 0x0000_0004; // CTRL bit 2
const STATUS_GIO_MASTER_ENABLE: u32 = 0x0008_0000; // STATUS bit 19
const MASTER_DISABLE_TIMEOUT: u32 = 800;

// Kumeran (KMRN) register access + K1 config
const E1000E_KMRNCTRLSTA: usize = 0x0034 / 4;
const KMRNCTRLSTA_OFFSET_SHIFT: u32 = 16;
const KMRNCTRLSTA_OFFSET: u32 = 0x001F_0000;
const KMRNCTRLSTA_REN: u32 = 0x0020_0000;
const KMRNCTRLSTA_K1_CONFIG: u32 = 0x7;
const KMRNCTRLSTA_K1_ENABLE: u16 = 0x0002;
const CTRL_EXT_SPD_BYPS: u32 = 0x0000_8000;
const CTRL_SPD_1000: u32 = 0x0000_0200;
const CTRL_SPD_100: u32 = 0x0000_0100;

// LANPHYPC toggle — re-powers the PHY after it leaves ULP
const CTRL_LANPHYPC_OVERRIDE: u32 = 0x0001_0000; // CTRL bit 16
const CTRL_LANPHYPC_VALUE: u32 = 0x0002_0000; // CTRL bit 17
const CTRL_EXT_LPCD: u32 = 0x0000_0004; // CTRL_EXT bit 2 (link phy config done)

// MII BMCR (PHY register 0) — IEEE standard autoneg bits
const MII_CR_RESTART_AUTO_NEG: u16 = 0x0200;
const MII_CR_AUTO_NEG_EN: u16 = 0x1000;

// Legacy RX descriptor status (LK / 8254x §3.2.3.1)
const RXD_STAT_DD: u8 = 1 << 0;
const RXD_STAT_EOP: u8 = 1 << 1;

// TX descriptor CMD bits
const TX_CMD_EOP: u8 = 1 << 0;
const TX_CMD_IFCS: u8 = 1 << 1;
const TX_CMD_RS: u8 = 1 << 3;

// TX descriptor STATUS bits (written back by hardware when CMD.RS is set)
const TX_STAT_DD: u8 = 1 << 0;

// DMA ring sizing
const NUM_RX: usize = 256;
const NUM_TX: usize = 256;
/// LK uses 2048-byte RX buffers; one slot holds a full MTU frame.
const BUF_SIZE: usize = 2048;
/// Cap on a reassembled multi-descriptor RX frame. Each non-EOP fragment
/// fills a full `BUF_SIZE` buffer, so the cap must be a multiple of it large
/// enough to hold several fragments — capping at `BUF_SIZE` itself (the
/// previous value) made the merge branch in `process_rx_slot` unreachable: a
/// second fragment's length always overflowed the already-accumulated first
/// buffer, so every multi-descriptor frame was silently dropped. 16 buffers
/// covers well past a 9K jumbo frame, should RCTL.LPE / SRRCTL buffer size
/// ever change from today's single-descriptor (<=1522B) configuration.
const MAX_RX_FRAME_BYTES: usize = BUF_SIZE * 16;
const RX_DRAIN_BUDGET: usize = 64;
/// Bounded retry budget for [`E1000eTxToken::consume`] when the TX ring is
/// momentarily full. The NIC drains the ring autonomously via DMA (TDH advances
/// without CPU help), so re-reading TDH a few times lets an in-flight frame
/// complete before we give up. Each retry re-reads TDH over MMIO (~1 µs on real
/// hardware) and a single 1514-byte frame clears the wire in ~12 µs at 1 Gbps,
/// so this bounds the wait to a few milliseconds — enough to never drop a pure
/// ACK / window-update under an RX burst, without spinning unboundedly if TX is
/// genuinely wedged.
const TX_SEND_SPIN_LIMIT: usize = 4096;
const DMA_RING_BYTES: usize = NUM_RX * size_of::<RxDesc>();
const DMA_TX_RING_BYTES: usize = NUM_TX * size_of::<TxDesc>();
const DMA_DESC_ALIGN: usize = 16;
const CACHE_LINE_SIZE: usize = 64;

// ---------------------------------------------------------------------------
// Descriptor layouts
// ---------------------------------------------------------------------------

/// Legacy RX descriptor (LK `rdesc` / Eclipse `E1000RecvDesc`).
#[repr(C, align(16))]
#[derive(Copy, Clone, Default)]
struct RxDesc {
    addr: u64,
    len: u16,
    chksum: u16,
    status: u8,
    errors: u8,
    vlan: u16,
}
const _RX_DESC_SIZE: () = assert!(core::mem::size_of::<RxDesc>() == 16);

/// Legacy TX descriptor (16 bytes).
#[repr(C, align(16))]
#[derive(Copy, Clone, Default)]
struct TxDesc {
    addr: u64,
    len: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}
const _TX_DESC_SIZE: () = assert!(core::mem::size_of::<TxDesc>() == 16);

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------

#[inline(always)]
unsafe fn mmio_read(base: usize, reg: usize) -> u32 {
    read_volatile((base + reg * 4) as *const u32)
}

#[inline(always)]
unsafe fn mmio_write(base: usize, reg: usize, val: u32) {
    write_volatile((base + reg * 4) as *mut u32, val);
}

/// True if `frame` is an IPv4 Ethernet frame whose IP *header* checksum is wrong
/// (the one-complement sum of the header 16-bit words must be 0xffff). Returns
/// false for non-IPv4 frames (ARP/IPv6) so they are not counted as corrupt.
fn rx_ipv4_hdr_csum_bad(frame: &[u8]) -> bool {
    if frame.len() < 14 + 20 || frame[12] != 0x08 || frame[13] != 0x00 {
        return false; // too short or not IPv4 (EtherType != 0x0800)
    }
    let ihl = (frame[14] & 0x0f) as usize * 4;
    if ihl < 20 || frame.len() < 14 + ihl {
        return false;
    }
    let mut sum: u32 = 0;
    let mut i = 14;
    while i < 14 + ihl {
        sum += ((frame[i] as u32) << 8) | frame[i + 1] as u32;
        i += 2;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum != 0xffff
}

// ---------------------------------------------------------------------------
// E1000eHw — hardware state
// ---------------------------------------------------------------------------

pub struct E1000eHw {
    base: usize,
    pci_loc: Location,
    device_id: u16,

    mac: [u8; 6],

    rx_ring: DmaRegion,
    rx_buf_pool: DmaRegion,
    rx_ring_coherent: bool,
    rx_buf_coherent: bool,
    /// LK `rx_last_head_`: next RX descriptor to inspect.
    rx_next_to_clean: usize,
    /// Multi-descriptor frame being reassembled (LK `rx_pending_pkt_`).
    rx_pending: Option<Vec<u8>>,
    /// True when one or more descriptors have been recycled since the last
    /// [`flush_rx_doorbell`](Self::flush_rx_doorbell) call. Ringing the RDT
    /// doorbell is an MMIO write (plus, previously, a synchronous readback);
    /// batching it across a whole receive burst instead of once per packet
    /// is what actually matters under load — see `flush_rx_doorbell`.
    rx_doorbell_dirty: bool,

    tx_ring: DmaRegion,
    tx_buf_pool: DmaRegion,
    tx_ring_coherent: bool,
    tx_buf_coherent: bool,
    tx_tail: usize,

    pub stats: NetStats,
    /// Count of received IPv4 frames whose header checksum did not verify —
    /// i.e. corrupted before smoltcp could parse them. Surfaced in the
    /// watchdog. Per-instance (not a file-level static) so two e1000e NICs
    /// don't conflate each other's diagnostics.
    rx_csum_bad: u64,
    /// Count of outgoing frames smoltcp handed us that we had to DROP because
    /// the TX ring stayed full past [`TX_SEND_SPIN_LIMIT`]. Any non-zero
    /// value means a pure ACK / window-update was lost — the exact cause of
    /// the silent download deadlock — so the watchdog surfaces it. Should
    /// stay 0 once TX completes. Per-instance for the same reason as above.
    tx_dropped: u64,

    link_up: bool,
    link_watchdog_next_us: u64,
    watchdog_log_next_us: u64,
    itr_setting: u32,
    itr_last_rx_packets: u64,
    itr_tune_next_us: u64,
}

impl E1000eHw {
    // -----------------------------------------------------------------------
    // Timing
    // -----------------------------------------------------------------------

    fn udelay(us: u64) {
        if us == 0 {
            return;
        }
        let t0 = timer_now_as_micros();
        const MAX_SPINS: u64 = 10_000_000;
        let mut n = 0u64;
        while timer_now_as_micros().wrapping_sub(t0) < us {
            core::hint::spin_loop();
            n += 1;
            if n >= MAX_SPINS {
                break;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Buffer address helpers
    // -----------------------------------------------------------------------

    #[inline]
    fn rx_buf_paddr(&self, i: usize) -> u64 {
        (self.rx_buf_pool.paddr() + i * BUF_SIZE) as u64
    }
    #[inline]
    fn rx_buf_vaddr(&self, i: usize) -> usize {
        self.rx_buf_pool.vaddr() + i * BUF_SIZE
    }
    #[inline]
    fn tx_buf_paddr(&self, i: usize) -> u64 {
        (self.tx_buf_pool.paddr() + i * BUF_SIZE) as u64
    }
    #[inline]
    fn tx_buf_vaddr(&self, i: usize) -> usize {
        self.tx_buf_pool.vaddr() + i * BUF_SIZE
    }

    // -----------------------------------------------------------------------
    // Device family helpers
    // -----------------------------------------------------------------------

    fn is_pch(&self) -> bool {
        // I217 (PCH-LPT), I218, I219 (PCH-SPT+) — all PCH-integrated NICs
        matches!(self.device_id,
            0x1502 | 0x1503 |                       // I82579
            0x153a | 0x153b |                       // I217
            0x155a | 0x1559 |                       // I218-LM/V (PCH-LPT)
            0x15a0..=0x15a3 |                       // I218-x (PCH-LPT)
            0x156f | 0x1570 |                       // I219-LM/V (PCH-SPT step A)
            0x15b7..=0x15be |                       // I219-x (PCH-SPT / KBP)
            0x15d6..=0x15d8 |                       // I219-x (PCH-CNP)
            0x15e3 |
            0x0d4c..=0x0d4f |
            0x15f4..=0x15fc |
            0x1a1c..=0x1a1f |
            0x0dc5..=0x0dc8 |
            0x550a..=0x5511 |
            0x57a0 | 0x57a1 |
            0x57b3..=0x57ba |
            0x15df..=0x15e2 |
            0x0d53 | 0x0d55
        )
    }

    fn is_pch_spt_or_later(&self) -> bool {
        // PCH-SPT (Sunrise Point, Kaby Lake, Coffee Lake, …) — device 0x156f+
        matches!(self.device_id,
            0x156f | 0x1570 |
            0x15b7..=0x15be |
            0x15d6..=0x15d8 |
            0x15e3 |
            0x0d4c..=0x0d4f |
            0x15f4..=0x15fc |
            0x1a1c..=0x1a1f |
            0x0dc5..=0x0dc8 |
            0x550a..=0x5511 |
            0x57a0 | 0x57a1 |
            0x57b3..=0x57ba |
            0x15df..=0x15e2 |
            0x0d53 | 0x0d55
        )
    }

    // -----------------------------------------------------------------------
    // MDIC (MDIO) — used only for PHY soft reset
    // -----------------------------------------------------------------------

    unsafe fn mdic_write(&self, phy_addr: u8, reg: u32, val: u16) -> bool {
        let cmd = (val as u32)
            | (reg << MDIC_REG_SHIFT)
            | ((phy_addr as u32) << MDIC_PHYADD_SHIFT)
            | MDIC_OP_WRITE;
        mmio_write(self.base, E1000E_MDIC, cmd);
        for _ in 0..MDIC_POLL_TRIES {
            Self::udelay(50);
            let v = mmio_read(self.base, E1000E_MDIC);
            if v & MDIC_READY != 0 {
                return v & MDIC_ERROR == 0;
            }
        }
        false
    }

    unsafe fn mdic_read(&self, phy_addr: u8, reg: u32) -> Option<u16> {
        let cmd = (reg << MDIC_REG_SHIFT) | ((phy_addr as u32) << MDIC_PHYADD_SHIFT) | MDIC_OP_READ;
        mmio_write(self.base, E1000E_MDIC, cmd);
        for _ in 0..MDIC_POLL_TRIES {
            Self::udelay(50);
            let v = mmio_read(self.base, E1000E_MDIC);
            if v & MDIC_READY != 0 {
                if v & MDIC_ERROR != 0 {
                    return None;
                }
                return Some(v as u16);
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // SW/FW semaphore (EXTCNF_CTRL.SWFLAG) — required before touching the PHY
    // on PCH parts while the ME firmware is active.
    // -----------------------------------------------------------------------

    unsafe fn acquire_swflag(&self) -> bool {
        let mut ext;
        let mut timeout = 100u32;
        loop {
            ext = mmio_read(self.base, E1000E_EXTCNF_CTRL);
            if ext & EXTCNF_CTRL_SWFLAG == 0 {
                break;
            }
            if timeout == 0 {
                return false;
            }
            Self::udelay(1_000);
            timeout -= 1;
        }
        ext |= EXTCNF_CTRL_SWFLAG;
        mmio_write(self.base, E1000E_EXTCNF_CTRL, ext);
        let mut timeout = 1_000u32;
        loop {
            ext = mmio_read(self.base, E1000E_EXTCNF_CTRL);
            if ext & EXTCNF_CTRL_SWFLAG != 0 {
                return true;
            }
            if timeout == 0 {
                ext &= !EXTCNF_CTRL_SWFLAG;
                mmio_write(self.base, E1000E_EXTCNF_CTRL, ext);
                return false;
            }
            Self::udelay(1_000);
            timeout -= 1;
        }
    }

    unsafe fn release_swflag(&self) {
        let ext = mmio_read(self.base, E1000E_EXTCNF_CTRL) & !EXTCNF_CTRL_SWFLAG;
        mmio_write(self.base, E1000E_EXTCNF_CTRL, ext);
    }

    // -----------------------------------------------------------------------
    // HV PHY paged register access (used only in the SW disable-ULP path).
    // Caller must hold the SW/FW semaphore.
    // -----------------------------------------------------------------------

    unsafe fn phy_read_hv(&self, page: u32, reg: u32) -> Option<u16> {
        if reg > MAX_PHY_MULTI_PAGE_REG
            && !self.mdic_write(
                HV_PHY_ADDR,
                PHY_PAGE_SELECT_REG,
                (page << PHY_PAGE_SHIFT) as u16,
            )
        {
            return None;
        }
        self.mdic_read(HV_PHY_ADDR, reg & MAX_PHY_REG_ADDRESS)
    }

    unsafe fn phy_write_hv(&self, page: u32, reg: u32, val: u16) -> bool {
        if reg > MAX_PHY_MULTI_PAGE_REG
            && !self.mdic_write(
                HV_PHY_ADDR,
                PHY_PAGE_SELECT_REG,
                (page << PHY_PAGE_SHIFT) as u16,
            )
        {
            return false;
        }
        self.mdic_write(HV_PHY_ADDR, reg & MAX_PHY_REG_ADDRESS, val)
    }

    // -----------------------------------------------------------------------
    // Disable ULP — port of Linux e1000_disable_ulp_lpt_lp().
    // On real i219 with active ME firmware the FW-handshake path runs (MMIO
    // only, no PHY access). The SW path is the fallback when no FW is present.
    // -----------------------------------------------------------------------

    unsafe fn disable_ulp(&self, force: bool) {
        if !self.is_pch_spt_or_later() {
            return;
        }

        let fwsm = mmio_read(self.base, E1000E_FWSM);
        if fwsm & ICH_FWSM_FW_VALID != 0 {
            // Firmware handshake path — ask the ME to un-configure ULP.
            if force {
                let mut h2me = mmio_read(self.base, E1000E_H2ME);
                h2me &= !H2ME_ULP;
                h2me |= H2ME_ENFORCE_SETTINGS;
                mmio_write(self.base, E1000E_H2ME, h2me);
            }
            // Poll up to ~400 ms for ME to clear ULP_CFG_DONE.
            let mut cleared = false;
            for _ in 0..40u32 {
                if mmio_read(self.base, E1000E_FWSM) & FWSM_ULP_CFG_DONE == 0 {
                    cleared = true;
                    break;
                }
                Self::udelay(10_000);
            }
            let mut h2me = mmio_read(self.base, E1000E_H2ME);
            if force {
                h2me &= !H2ME_ENFORCE_SETTINGS;
            } else {
                h2me &= !H2ME_ULP;
            }
            mmio_write(self.base, E1000E_H2ME, h2me);
            crate::klog_warn!(
                "[e1000e] disable_ulp FW-path cfg_done_cleared={}\n",
                cleared
            );
            return;
        }

        // Software path — drive the PHY directly (no ME firmware present).
        if !self.acquire_swflag() {
            crate::klog_warn!("[e1000e] disable_ulp: SW/FW semaphore busy\n");
            return;
        }
        // Clear FORCE_SMBUS in the PHY.
        if let Some(mut p) = self.phy_read_hv(CV_SMB_CTRL_PAGE, CV_SMB_CTRL_REG) {
            p &= !CV_SMB_CTRL_FORCE_SMBUS;
            let _ = self.phy_write_hv(CV_SMB_CTRL_PAGE, CV_SMB_CTRL_REG, p);
        }
        // Unforce SMBus at the MAC.
        let ext = mmio_read(self.base, E1000E_CTRL_EXT) & !CTRL_EXT_FORCE_SMBUS;
        mmio_write(self.base, E1000E_CTRL_EXT, ext);
        // Re-enable K1 (ME disables it when entering ULP).
        if let Some(mut p) = self.phy_read_hv(HV_PM_CTRL_PAGE, HV_PM_CTRL_REG) {
            p |= HV_PM_CTRL_K1_ENABLE;
            let _ = self.phy_write_hv(HV_PM_CTRL_PAGE, HV_PM_CTRL_REG, p);
        }
        // Clear the ULP configuration and commit (START).
        if let Some(mut p) = self.phy_read_hv(ULP_CONFIG1_PAGE, ULP_CONFIG1_REG) {
            p &= !(ULP_CONFIG1_IND
                | ULP_CONFIG1_STICKY_ULP
                | ULP_CONFIG1_RESET_TO_SMBUS
                | ULP_CONFIG1_WOL_HOST
                | ULP_CONFIG1_INBAND_EXIT
                | ULP_CONFIG1_DISABLE_SMB_PERST);
            let _ = self.phy_write_hv(ULP_CONFIG1_PAGE, ULP_CONFIG1_REG, p);
            p |= ULP_CONFIG1_START;
            let _ = self.phy_write_hv(ULP_CONFIG1_PAGE, ULP_CONFIG1_REG, p);
        }
        // Clear FEXTNVM7.DISABLE_SMB_PERST.
        let f7 = mmio_read(self.base, E1000E_FEXTNVM7) & !FEXTNVM7_DISABLE_SMB_PERST;
        mmio_write(self.base, E1000E_FEXTNVM7, f7);
        self.release_swflag();
        crate::klog_warn!("[e1000e] disable_ulp SW-path done\n");
    }

    // -----------------------------------------------------------------------
    // GIO master disable — quiesce in-flight DMA before CTRL_RST so the reset
    // doesn't fire while the device is mastering the bus (port of
    // e1000e_disable_pcie_master). Returns false if requests stay pending.
    // -----------------------------------------------------------------------

    unsafe fn disable_pcie_master(&self) -> bool {
        let ctrl = mmio_read(self.base, E1000E_CTRL) | CTRL_GIO_MASTER_DISABLE;
        mmio_write(self.base, E1000E_CTRL, ctrl);
        for _ in 0..MASTER_DISABLE_TIMEOUT {
            if mmio_read(self.base, E1000E_STATUS) & STATUS_GIO_MASTER_ENABLE == 0 {
                return true;
            }
            Self::udelay(100);
        }
        false
    }

    // -----------------------------------------------------------------------
    // Kumeran (KMRN) register access and K1 power-state config
    // (port of e1000_configure_k1_ich8lan). Caller need not hold the semaphore;
    // configure_k1 takes it internally.
    // -----------------------------------------------------------------------

    unsafe fn kmrn_read(&self, offset: u32) -> u16 {
        let cmd = ((offset << KMRNCTRLSTA_OFFSET_SHIFT) & KMRNCTRLSTA_OFFSET) | KMRNCTRLSTA_REN;
        mmio_write(self.base, E1000E_KMRNCTRLSTA, cmd);
        let _ = mmio_read(self.base, E1000E_STATUS); // flush
        Self::udelay(2);
        mmio_read(self.base, E1000E_KMRNCTRLSTA) as u16
    }

    unsafe fn kmrn_write(&self, offset: u32, data: u16) {
        let cmd = ((offset << KMRNCTRLSTA_OFFSET_SHIFT) & KMRNCTRLSTA_OFFSET) | data as u32;
        mmio_write(self.base, E1000E_KMRNCTRLSTA, cmd);
        let _ = mmio_read(self.base, E1000E_STATUS); // flush
        Self::udelay(2);
    }

    unsafe fn configure_k1(&self, enable: bool) {
        if !self.is_pch() {
            return;
        }
        if !self.acquire_swflag() {
            crate::klog_warn!("[e1000e] configure_k1: SW/FW semaphore busy\n");
            return;
        }
        let mut kmrn = self.kmrn_read(KMRNCTRLSTA_K1_CONFIG);
        if enable {
            kmrn |= KMRNCTRLSTA_K1_ENABLE;
        } else {
            kmrn &= !KMRNCTRLSTA_K1_ENABLE;
        }
        self.kmrn_write(KMRNCTRLSTA_K1_CONFIG, kmrn);
        Self::udelay(30);

        let ctrl_ext = mmio_read(self.base, E1000E_CTRL_EXT);
        let ctrl_reg = mmio_read(self.base, E1000E_CTRL);
        let mut reg = ctrl_reg & !(CTRL_SPD_1000 | CTRL_SPD_100);
        reg |= CTRL_FRCSPD | CTRL_FRCDPX;
        mmio_write(self.base, E1000E_CTRL, reg);
        mmio_write(self.base, E1000E_CTRL_EXT, ctrl_ext | CTRL_EXT_SPD_BYPS);
        let _ = mmio_read(self.base, E1000E_STATUS); // flush
        Self::udelay(30);
        // Restore CTRL / CTRL_EXT to the pre-K1 values (preserves our SLU+ASDE).
        mmio_write(self.base, E1000E_CTRL, ctrl_reg);
        mmio_write(self.base, E1000E_CTRL_EXT, ctrl_ext);
        let _ = mmio_read(self.base, E1000E_STATUS); // flush
        Self::udelay(30);

        self.release_swflag();
    }

    // -----------------------------------------------------------------------
    // LANPHYPC toggle — force the PHY to re-run its power-up/config sequence
    // after leaving ULP (port of e1000_toggle_lanphypc_pch_lpt). MMIO only.
    // -----------------------------------------------------------------------

    unsafe fn toggle_lanphypc(&self) {
        if !self.is_pch() {
            return;
        }
        // Toggle LANPHYPC value bit with override asserted, then deasserted.
        let mut ctrl = mmio_read(self.base, E1000E_CTRL);
        ctrl |= CTRL_LANPHYPC_OVERRIDE;
        ctrl &= !CTRL_LANPHYPC_VALUE;
        mmio_write(self.base, E1000E_CTRL, ctrl);
        let _ = mmio_read(self.base, E1000E_STATUS); // flush
        Self::udelay(20);
        ctrl &= !CTRL_LANPHYPC_OVERRIDE;
        mmio_write(self.base, E1000E_CTRL, ctrl);
        let _ = mmio_read(self.base, E1000E_STATUS); // flush

        if self.is_pch_spt_or_later() {
            // PCH-LPT+: wait for the PHY config-done indication (LPCD), ~120 ms max.
            let mut count = 20u16;
            loop {
                Self::udelay(6_000);
                if mmio_read(self.base, E1000E_CTRL_EXT) & CTRL_EXT_LPCD != 0 {
                    break;
                }
                if count == 0 {
                    break;
                }
                count -= 1;
            }
            Self::udelay(30_000);
        } else {
            Self::udelay(50_000);
        }
    }

    // -----------------------------------------------------------------------
    // Restart auto-negotiation via the PHY BMCR (register 0). Tries both
    // possible PHY addresses and protects the access with the SW/FW semaphore.
    // -----------------------------------------------------------------------

    unsafe fn restart_autoneg(&self) {
        if !self.acquire_swflag() {
            return;
        }
        for phy_addr in [1u8, 2u8] {
            if let Some(bmcr) = self.mdic_read(phy_addr, 0) {
                if bmcr == 0xFFFF {
                    continue;
                }
                let v = bmcr | MII_CR_AUTO_NEG_EN | MII_CR_RESTART_AUTO_NEG;
                if self.mdic_write(phy_addr, 0, v) {
                    crate::klog_warn!("[e1000e] restart autoneg on phy_addr={}\n", phy_addr);
                    break;
                }
            }
        }
        self.release_swflag();
    }

    // -----------------------------------------------------------------------
    // PHY soft reset — clears all PHY registers to power-on defaults
    // (including any BM WUC filters left by firmware)
    // -----------------------------------------------------------------------

    unsafe fn phy_soft_reset(&self) {
        // Try both possible PHY addresses
        for phy_addr in [1u8, 2u8] {
            // Read BMSR to check if PHY is present
            if self.mdic_read(phy_addr, 1).is_none() {
                continue;
            }
            // Write BMCR reset bit
            let _ = self.mdic_write(phy_addr, 0, BMCR_RESET);
            // Wait for reset to complete (up to 500ms)
            for _ in 0..500 {
                Self::udelay(1000);
                if let Some(bmcr) = self.mdic_read(phy_addr, 0) {
                    if bmcr & BMCR_RESET == 0 {
                        break;
                    }
                }
            }
        }
        // Allow PHY to settle
        Self::udelay(10_000);
    }

    // -----------------------------------------------------------------------
    // MAC address
    // -----------------------------------------------------------------------

    unsafe fn read_mac_from_hw(&mut self) {
        let ral = mmio_read(self.base, E1000E_RAL0);
        let rah = mmio_read(self.base, E1000E_RAH0);
        if ral == 0 && (rah & 0xFFFF) == 0 {
            // Try EERD as fallback
            self.read_mac_from_eeprom();
            return;
        }
        self.mac[0] = (ral & 0xFF) as u8;
        self.mac[1] = ((ral >> 8) & 0xFF) as u8;
        self.mac[2] = ((ral >> 16) & 0xFF) as u8;
        self.mac[3] = ((ral >> 24) & 0xFF) as u8;
        self.mac[4] = (rah & 0xFF) as u8;
        self.mac[5] = ((rah >> 8) & 0xFF) as u8;
    }

    unsafe fn read_mac_from_eeprom(&mut self) {
        for word in 0..3u16 {
            let w = self.eerd_read(word);
            if w == 0 || w == 0xFFFF {
                continue;
            }
            self.mac[(word as usize) * 2] = (w & 0xFF) as u8;
            self.mac[(word as usize) * 2 + 1] = (w >> 8) as u8;
        }
    }

    unsafe fn eerd_read(&self, offset: u16) -> u16 {
        // Try shift-2 (most discrete e1000e)
        for shift in [2u32, 3u32] {
            let cmd = ((offset as u32) << shift) | EERD_START;
            mmio_write(self.base, E1000E_EERD, cmd);
            for _ in 0..2000u32 {
                Self::udelay(50);
                let v = mmio_read(self.base, E1000E_EERD);
                if v & (EERD_DONE_BIT4 | EERD_DONE_BIT1) != 0 {
                    return (v >> EERD_DATA_SHIFT) as u16;
                }
            }
        }
        0
    }

    fn is_valid_mac(&self) -> bool {
        let all_zeros = self.mac.iter().all(|&b| b == 0);
        let all_ff = self.mac.iter().all(|&b| b == 0xFF);
        !all_zeros && !all_ff
    }

    // -----------------------------------------------------------------------
    // Main init — reset, configure, arm rings
    // -----------------------------------------------------------------------

    pub unsafe fn reset_and_init(&mut self) -> DeviceResult<()> {
        // 1. Mask all interrupts
        mmio_write(self.base, E1000E_IMC, 0xFFFF_FFFF);
        let _ = mmio_read(self.base, E1000E_IMC);

        // 2. Disable RX / TX
        mmio_write(self.base, E1000E_RCTL, 0);
        mmio_write(self.base, E1000E_TCTL, TCTL_PSP);
        let _ = mmio_read(self.base, E1000E_STATUS);
        Self::udelay(10_000);

        // 3. Disable queue enables (I219 SPT must clear QUEUE_ENABLE before CTRL_RST)
        if self.is_pch_spt_or_later() {
            let rxdctl = mmio_read(self.base, E1000E_RXDCTL);
            mmio_write(self.base, E1000E_RXDCTL, rxdctl & !RXDCTL_QUEUE_ENABLE);
            let txdctl = mmio_read(self.base, E1000E_TXDCTL);
            mmio_write(self.base, E1000E_TXDCTL, txdctl & !TXDCTL_QUEUE_ENABLE);
            let _ = mmio_read(self.base, E1000E_STATUS);
            Self::udelay(1_000);
        }

        // 4. Clear WUC/WUFC so PHY WUC filter is disabled at the MAC level too
        mmio_write(self.base, E1000E_WUC, 0);
        mmio_write(self.base, E1000E_WUFC, 0);

        // 4.5 Disable ULP (i219 real hardware): bring the PHY out of Ultra Low
        //     Power mode so auto-negotiation can run and STATUS.LU can assert.
        //     No-op on QEMU/discrete parts (no ME firmware, not PCH-SPT).
        self.disable_ulp(true);

        // 4.6 Toggle LANPHYPC so the PHY re-runs its power-up/config sequence
        //     now that it is out of ULP.
        self.toggle_lanphypc();

        // 4.7 Quiesce in-flight DMA before resetting (GIO master disable).
        if !self.disable_pcie_master() {
            crate::klog_warn!("[e1000e] GIO master requests still pending before reset\n");
        }

        // 5. MAC reset (CTRL_RST)
        {
            let ctrl = mmio_read(self.base, E1000E_CTRL);
            mmio_write(self.base, E1000E_CTRL, ctrl | CTRL_RST);
        }
        // Wait for reset to self-clear
        for _ in 0..1000u32 {
            Self::udelay(1_000);
            if mmio_read(self.base, E1000E_CTRL) & CTRL_RST == 0 {
                break;
            }
        }
        Self::udelay(10_000);

        // 6. Mask interrupts again (reset clears IMC)
        mmio_write(self.base, E1000E_IMC, 0xFFFF_FFFF);
        let _ = mmio_read(self.base, E1000E_ICR);

        // 7. Re-enable PCI bus master (CTRL_RST may disable it on I219)
        {
            let mut cmd = PCI_ACCESS.read16(&PortOpsImpl, self.pci_loc, 0x04);
            cmd |= 0x0004 | 0x0002; // Bus Master + Memory Space
            PCI_ACCESS.write16(&PortOpsImpl, self.pci_loc, 0x04, cmd);
        }

        // 8. CTRL_EXT: disable PCIe relaxed ordering, signal driver loaded.
        //    LK enables IAME on e1000e (QEMU 82574); keep it off on PCH i219.
        {
            let mut ext = mmio_read(self.base, E1000E_CTRL_EXT);
            ext |= CTRL_EXT_RO_DIS | CTRL_EXT_DRV_LOAD;
            if self.is_pch() {
                ext &= !CTRL_EXT_IAME;
            } else {
                ext |= CTRL_EXT_IAME;
            }
            mmio_write(self.base, E1000E_CTRL_EXT, ext);
            let _ = mmio_read(self.base, E1000E_CTRL_EXT);
            if !self.is_pch() {
                mmio_write(self.base, E1000E_IAM, 0);
            }
        }

        // 9. FEXTNVM6/7 workarounds for PCH-SPT (Linux ich8lan.c)
        if self.is_pch_spt_or_later() {
            let fext6 = mmio_read(self.base, E1000E_FEXTNVM6);
            mmio_write(self.base, E1000E_FEXTNVM6, fext6 & !0x0000_0010); // clear bit 4
            let fext7 = mmio_read(self.base, E1000E_FEXTNVM7);
            mmio_write(self.base, E1000E_FEXTNVM7, fext7 | 0x0000_0001); // set bit 0
        }

        // 10. Disable LPLU via MAC PHY_CTRL register (no MDIO needed)
        if self.is_pch() {
            let mut phy_ctrl = mmio_read(self.base, E1000E_PHY_CTRL);
            phy_ctrl &= !(PHY_CTRL_D0A_LPLU
                | PHY_CTRL_NOND0A_LPLU
                | PHY_CTRL_GBE_DISABLE
                | PHY_CTRL_NOND0A_GBE_DISABLE);
            mmio_write(self.base, E1000E_PHY_CTRL, phy_ctrl);
            let _ = mmio_read(self.base, E1000E_PHY_CTRL);
            Self::udelay(1_000);
        }

        // 11. Skip PHY soft reset — BMCR reset disrupts auto-negotiation (3-5 s) and may
        //     reload LPLU from NVM, permanently keeping the link down. The MAC-level
        //     CTRL_RST + PHY_CTRL LPLU clear is sufficient; the OSDev i219-V guide
        //     confirms this works without any PHY soft reset on real hardware.

        // 12. CTRL: SLU + ASDE, clear force-speed/duplex
        {
            let mut ctrl = mmio_read(self.base, E1000E_CTRL);
            ctrl &= !(CTRL_FRCSPD | CTRL_FRCDPX);
            ctrl |= CTRL_SLU | CTRL_ASDE;
            mmio_write(self.base, E1000E_CTRL, ctrl);
            let _ = mmio_read(self.base, E1000E_CTRL);
        }

        // 12.5 Configure K1 (Kumeran power state) to a known-good enabled state.
        //      Runs the FRCSPD/SPD_BYPS dance from Linux and restores CTRL.
        self.configure_k1(true);

        // 12.7 Kick off auto-negotiation explicitly via the PHY BMCR, in case
        //      SLU+ASDE alone didn't restart it after the ULP/LANPHYPC dance.
        self.restart_autoneg();

        // 13. Read MAC address
        self.read_mac_from_hw();
        if !self.is_valid_mac() {
            crate::klog_warn!("[e1000e] MAC all-zero/FF after reset — using placeholder\n");
            self.mac = [0x00, 0x0E, 0x10, 0xDE, 0xAD, 0x01];
        }

        // 14. Clear MTA (multicast table)
        for i in 0..128usize {
            mmio_write(self.base, E1000E_MTA_BASE + i, 0);
        }

        // 15. Clear VLAN filter table
        for i in 0..128usize {
            mmio_write(self.base, E1000E_VFTA_BASE + i, 0);
        }

        // 16. Disable VET (VLAN EtherType — use 0 for untagged)
        mmio_write(self.base, E1000E_VET, 0);

        // 17. Disable WUC at MAC level
        mmio_write(self.base, E1000E_WUC, 0);
        mmio_write(self.base, E1000E_WUFC, 0);

        // 18. Program TX ring
        self.init_tx();

        // 19. Program RX ring and enable
        self.init_rx();

        // 20. Enable interrupts
        compiler_fence(Ordering::SeqCst);
        mmio_write(self.base, E1000E_IMS, IMS_REARM);
        let _ = mmio_read(self.base, E1000E_IMS);

        // 21. Check link immediately
        let status = mmio_read(self.base, E1000E_STATUS);
        self.link_up = status & STATUS_LU != 0;
        crate::klog_warn!(
            "[e1000e] reset_and_init done: STATUS={:#010x} LU={} GPRC={} tag={}\n",
            status,
            self.link_up,
            mmio_read(self.base, E1000E_GPRC),
            E1000E_DRIVER_TAG
        );

        Ok(())
    }

    unsafe fn init_tx(&mut self) {
        // Program TX ring base, length, head, tail
        let tx_pa = self.tx_ring.paddr();
        mmio_write(self.base, E1000E_TDBAL, tx_pa as u32);
        mmio_write(self.base, E1000E_TDBAH, (tx_pa >> 32) as u32);
        mmio_write(
            self.base,
            E1000E_TDLEN,
            (NUM_TX * size_of::<TxDesc>()) as u32,
        );
        mmio_write(self.base, E1000E_TDH, 0);
        mmio_write(self.base, E1000E_TDT, 0);
        self.tx_tail = 0;

        // Pre-mark every descriptor DD (done/free) so `can_send`/`send` can gate
        // on the real hardware-ownership bit from the very first transmit, the
        // same way `e1000.rs` does. Without this, freshly allocated (possibly
        // uninitialized) descriptor memory could read DD=0 and permanently look
        // "owned by hardware" even though it was never posted.
        {
            let ring = self.tx_ring.as_ptr::<TxDesc>();
            for i in 0..NUM_TX {
                let desc = unsafe { &mut *ring.add(i) };
                write_volatile(&mut desc.addr, self.tx_buf_paddr(i));
                write_volatile(&mut desc.len, 0);
                write_volatile(&mut desc.cso, 0);
                write_volatile(&mut desc.cmd, 0);
                write_volatile(&mut desc.status, TX_STAT_DD);
                write_volatile(&mut desc.css, 0);
                write_volatile(&mut desc.special, 0);
            }
            dma_sync_rx_desc_span(
                &self.tx_ring,
                self.tx_ring_coherent,
                0,
                NUM_TX,
                size_of::<TxDesc>(),
                DmaSyncDir::ToDevice,
            );
        }

        // Timers
        mmio_write(self.base, E1000E_TIDV, 0);
        mmio_write(self.base, E1000E_TADV, 0);

        // Inter-Packet Gap: IPGT=8, IPGR1=8, IPGR2=6 — the 8254x/e1000e
        // datasheet copper default (0x00602008). IPGR2 was previously 12,
        // doubling the half-duplex carrier-sense/defer window versus spec.
        mmio_write(self.base, E1000E_TIPG, 8 | (8 << 10) | (6 << 20));

        // TXDCTL
        if self.is_pch_spt_or_later() {
            // PCH-SPT: must set QUEUE_ENABLE (bit 25) and wait for it to latch
            let txdctl = TXDCTL_DMA_BURST | TXDCTL_QUEUE_ENABLE;
            mmio_write(self.base, E1000E_TXDCTL, txdctl);
            let mut latched = false;
            for _ in 0..100u32 {
                Self::udelay(100);
                if mmio_read(self.base, E1000E_TXDCTL) & TXDCTL_QUEUE_ENABLE != 0 {
                    latched = true;
                    break;
                }
            }
            if !latched {
                // TDH will never advance and every send() will spin-fail: this
                // was previously silent, so a dead TX queue looked identical
                // to a healthy but idle one until traffic was expected.
                crate::klog_warn!(
                    "[e1000e] TXDCTL.QUEUE_ENABLE did not latch within 10ms — TX queue may be dead\n"
                );
            }
            // Mirror to queue 1 (Linux e1000_configure_tx)
            mmio_write(
                self.base,
                E1000E_TXDCTL1,
                mmio_read(self.base, E1000E_TXDCTL),
            );
            // IOSF PCIe compliance
            let iosfpc = mmio_read(self.base, E1000E_IOSFPC);
            mmio_write(self.base, E1000E_IOSFPC, iosfpc | 0x0001_0000);
            let _ = mmio_read(self.base, E1000E_IOSFPC);
        } else {
            mmio_write(
                self.base,
                E1000E_TXDCTL,
                TXDCTL_DMA_BURST | TXDCTL_FULL_TX_DESC_WB,
            );
        }

        // TCTL: enable TX
        let tctl = TCTL_EN | TCTL_PSP | TCTL_RTLC | TCTL_CT_LINUX | TCTL_COLD_LINUX;
        mmio_write(self.base, E1000E_TCTL, tctl);
        let _ = mmio_read(self.base, E1000E_TCTL);
    }

    unsafe fn init_rx(&mut self) {
        // Timers off
        mmio_write(self.base, E1000E_RDTR, 0);
        mmio_write(self.base, E1000E_RADV, 0);
        self.program_itr(E1000E_ITR_BALANCED);

        // Program RX ring base, length, head
        let rx_pa = self.rx_ring.paddr();
        mmio_write(self.base, E1000E_RDBAL, rx_pa as u32);
        mmio_write(self.base, E1000E_RDBAH, (rx_pa >> 32) as u32);
        mmio_write(
            self.base,
            E1000E_RDLEN,
            (NUM_RX * size_of::<RxDesc>()) as u32,
        );
        mmio_write(self.base, E1000E_RDH, 0);
        self.rx_next_to_clean = 0;
        self.rx_pending = None;

        // Fill RX ring (LK: post buffer per slot, legacy descriptor layout).
        let ring = self.rx_ring.as_ptr::<RxDesc>();
        for i in 0..NUM_RX {
            let desc = unsafe { &mut *ring.add(i) };
            write_volatile(&mut desc.addr, self.rx_buf_paddr(i));
            write_volatile(&mut desc.len, 0);
            write_volatile(&mut desc.chksum, 0);
            write_volatile(&mut desc.status, 0);
            write_volatile(&mut desc.errors, 0);
            write_volatile(&mut desc.vlan, 0);
        }
        dma_sync_rx_desc_span(
            &self.rx_ring,
            self.rx_ring_coherent,
            0,
            NUM_RX,
            size_of::<RxDesc>(),
            DmaSyncDir::ToDevice,
        );
        compiler_fence(Ordering::SeqCst);
        fence(Ordering::SeqCst);

        // Legacy write-back only (LK / QEMU). Linux extended WB is for PCH+RFCTL_EXTEN.
        let mut rfctl = mmio_read(self.base, E1000E_RFCTL);
        rfctl &= !RFCTL_EXTEN;
        rfctl |= RFCTL_NFSW_DIS | RFCTL_NFSR_DIS;
        mmio_write(self.base, E1000E_RFCTL, rfctl);
        let _ = mmio_read(self.base, E1000E_RFCTL);

        // No multiqueue
        mmio_write(self.base, E1000E_MRQC, 0);

        // PCH: SRRCTL 2 KB + Drop_En. Discrete/QEMU: RCTL buffer size alone (LK).
        if self.is_pch() {
            mmio_write(self.base, E1000E_SRRCTL, 2 | (1 << 31));
        }

        // PCH-SPT: must set RXDCTL.QUEUE_ENABLE (bit 25) before RCTL.EN
        if self.is_pch_spt_or_later() {
            let rxdctl = mmio_read(self.base, E1000E_RXDCTL) | RXDCTL_QUEUE_ENABLE;
            mmio_write(self.base, E1000E_RXDCTL, rxdctl);
            for _ in 0..100u32 {
                Self::udelay(100);
                if mmio_read(self.base, E1000E_RXDCTL) & RXDCTL_QUEUE_ENABLE != 0 {
                    break;
                }
            }
        }

        // Doorbell: give (NUM_RX - 1) descriptors to hardware
        // RDT = last descriptor index hardware can use
        mmio_write(self.base, E1000E_RDT, (NUM_RX - 1) as u32);
        let _ = mmio_read(self.base, E1000E_RDT); // flush

        // Small settle before enabling RCTL
        Self::udelay(1_000);

        // RCTL: LK-style — EN, promisc, mcast promisc, broadcast, 2048-byte
        // buffers, strip Ethernet FCS. SECRC (bit 26) is a standard RCTL bit
        // across the whole 8254x/e1000e family, not a PCH-only feature — gating
        // it on is_pch() left every non-PCH part (82574L, and QEMU's e1000e
        // model) delivering 4 extra FCS/garbage bytes on every received frame.
        // `e1000.rs` sets it unconditionally for the same reason.
        let rctl = RCTL_EN | RCTL_UPE | RCTL_MPE | RCTL_BAM | RCTL_SECRC;
        mmio_write(self.base, E1000E_RCTL, rctl);
        let _ = mmio_read(self.base, E1000E_RCTL);

        compiler_fence(Ordering::SeqCst);
        fence(Ordering::SeqCst);
    }

    // -----------------------------------------------------------------------
    // RX data path (LK e1000: drain by RDH, legacy rdesc, fragment reassembly)
    // -----------------------------------------------------------------------

    fn rx_rdh(&self) -> usize {
        unsafe { mmio_read(self.base, E1000E_RDH) as usize }
    }

    fn clear_rx_pending(&mut self) {
        self.rx_pending = None;
    }

    /// LK `irq_handler` RXO path: drop any in-flight multi-descriptor frame.
    pub fn handle_rx_irq(&mut self, icr: u32) {
        if icr & ICR_RXO != 0 {
            if self.rx_pending.is_some() {
                self.clear_rx_pending();
                self.stats.rx_dropped += 1;
            }
            crate::klog_warn!("[e1000e] RX overrun (ICR_RXO)\n");
        }
    }

    /// Process one RX slot. Caller (`receive`) guarantees `rx_next_to_clean`
    /// is not caught up with RDH before calling — see the cached snapshot
    /// there; re-reading RDH here on every single slot (as this used to do)
    /// was the single largest per-packet MMIO cost in this path.
    fn process_rx_slot(&mut self) -> Option<Vec<u8>> {
        let head = self.rx_next_to_clean;

        dma_sync_rx_desc_span(
            &self.rx_ring,
            self.rx_ring_coherent,
            head,
            1,
            size_of::<RxDesc>(),
            DmaSyncDir::FromDevice,
        );

        // Copy descriptor locally (LK: consistent snapshot after dma_sync).
        let rxd = unsafe { read_volatile(self.rx_ring.as_ptr::<RxDesc>().add(head)) };

        // Never advance past a slot HW owns without DD — skipping desyncs the ring.
        if rxd.status & RXD_STAT_DD == 0 {
            return None;
        }

        fence(Ordering::Acquire);

        let len = rxd.len as usize;
        let eop = rxd.status & RXD_STAT_EOP != 0;
        let expected_addr = self.rx_buf_paddr(head);

        if rxd.addr != expected_addr {
            crate::klog_warn!(
                "[e1000e] RX addr mismatch slot={} desc={:#x} expected={:#x}\n",
                head,
                rxd.addr,
                expected_addr
            );
            self.clear_rx_pending();
            self.stats.rx_dropped += 1;
            self.rx_next_to_clean = (head + 1) % NUM_RX;
            unsafe {
                self.recycle_rx_slot(head);
            }
            return None;
        }

        if rxd.errors != 0 || len == 0 || len > BUF_SIZE {
            self.clear_rx_pending();
            self.stats.rx_dropped += 1;
            self.rx_next_to_clean = (head + 1) % NUM_RX;
            unsafe {
                self.recycle_rx_slot(head);
            }
            return None;
        }

        // Invalidate exactly the bytes we're about to read (`len`, rounded up
        // to a cache line by clflush_span), not the whole BUF_SIZE (2048)
        // buffer: the slice built below is `..len`, so nothing past it is
        // ever read. Flushing the full buffer on every packet regardless of
        // its actual size — up to 32 cache lines to read a 64-byte ACK — was
        // pure per-packet overhead with no correctness benefit.
        dma_sync_region(
            &self.rx_buf_pool,
            self.rx_buf_coherent,
            head * BUF_SIZE,
            len,
            DmaSyncDir::FromDevice,
        );
        let frag =
            unsafe { core::slice::from_raw_parts(self.rx_buf_vaddr(head) as *const u8, len) };

        let complete = if let Some(ref mut pending) = self.rx_pending {
            if pending.len().saturating_add(len) > MAX_RX_FRAME_BYTES {
                self.clear_rx_pending();
                self.stats.rx_dropped += 1;
                None
            } else {
                pending.extend_from_slice(frag);
                if eop {
                    Some(self.rx_pending.take().unwrap())
                } else {
                    None
                }
            }
        } else if eop {
            Some(frag.to_vec())
        } else {
            self.rx_pending = Some(frag.to_vec());
            None
        };

        if let Some(ref pkt) = complete {
            self.stats.rx_packets += 1;
            self.stats.rx_bytes += pkt.len() as u64;
        }

        self.rx_next_to_clean = (head + 1) % NUM_RX;
        unsafe {
            self.recycle_rx_slot(head);
        }
        complete
    }

    fn receive(&mut self) -> Option<Vec<u8>> {
        // Snapshot RDH once per call instead of re-reading it (a live MMIO
        // register) on every slot in the drain loop below — up to 2 reads per
        // iteration previously. A packet that races in after this snapshot is
        // simply picked up on the next `receive()` call; that's the normal
        // budget/yield behavior of a bounded drain, not a correctness issue.
        let rdh = self.rx_rdh();
        for _ in 0..RX_DRAIN_BUDGET {
            if self.rx_next_to_clean == rdh {
                // Caught up with HW as of this call's RDH snapshot.
                break;
            }
            let head_before = self.rx_next_to_clean;
            if let Some(pkt) = self.process_rx_slot() {
                // Corruption probe: an IPv4 frame whose header checksum does not
                // verify is corrupt before smoltcp ever sees it. smoltcp would
                // then silently drop it (no ACK) — exactly the "RX climbs, TX
                // freezes, peer keeps retransmitting" stall. If this counter
                // climbs while the download is stuck, the bytes are being
                // corrupted between the NIC DMA and here (memory corruption /
                // DMA), not a TCP-window issue.
                if rx_ipv4_hdr_csum_bad(&pkt) {
                    self.rx_csum_bad += 1;
                }
                return Some(pkt);
            }
            if self.rx_next_to_clean == head_before {
                // Slot not ready (DD clear) — wait for next poll.
                break;
            }
        }
        None
    }

    /// LK `add_pktbuf_to_rxring_locked`, minus the doorbell: refill the
    /// descriptor and mark the RDT doorbell dirty. Ringing it is deferred to
    /// [`flush_rx_doorbell`](Self::flush_rx_doorbell) — see there for why.
    unsafe fn recycle_rx_slot(&mut self, i: usize) {
        let ring = self.rx_ring.as_ptr::<RxDesc>();
        let desc = &mut *ring.add(i);
        write_volatile(&mut desc.status, 0);
        write_volatile(&mut desc.errors, 0);
        write_volatile(&mut desc.len, 0);
        write_volatile(&mut desc.chksum, 0);
        write_volatile(&mut desc.vlan, 0);
        write_volatile(&mut desc.addr, self.rx_buf_paddr(i));
        dma_sync_rx_desc_span(
            &self.rx_ring,
            self.rx_ring_coherent,
            i,
            1,
            size_of::<RxDesc>(),
            DmaSyncDir::ToDevice,
        );
        self.rx_doorbell_dirty = true;
    }

    /// Ring the RDT doorbell once for every descriptor recycled since the
    /// last flush, instead of on every single packet. This used to be a
    /// `mmio_write` immediately followed by a synchronous `mmio_read` on
    /// every recycled slot — one MMIO round trip per packet, plus a second
    /// live read of RDH per slot in the old `receive()` loop. On real
    /// hardware an MMIO read forces the CPU to stall for a PCIe completion;
    /// under emulation (QEMU) each MMIO access typically costs a full VM
    /// exit. Coalescing the doorbell across a whole receive burst — flushed
    /// once per poll cycle or `recv()` call by the caller, not per packet —
    /// is the difference between one doorbell ring per *packet* and one per
    /// *batch*. Deferring is safe: it only delays telling hardware about
    /// freed buffers, it never blocks forward progress on anything.
    pub fn flush_rx_doorbell(&mut self) {
        if !self.rx_doorbell_dirty {
            return;
        }
        self.rx_doorbell_dirty = false;
        let rdt = (self.rx_next_to_clean + NUM_RX - 1) % NUM_RX;
        unsafe {
            fence(Ordering::SeqCst);
            mmio_write(self.base, E1000E_RDT, rdt as u32);
        }
    }

    // -----------------------------------------------------------------------
    // TX data path
    // -----------------------------------------------------------------------

    /// Is the slot at `tx_tail` free to post into? Intel documents that TDH is
    /// NOT a reliable completion signal — it reflects descriptors the NIC has
    /// prefetched into its internal FIFO, which can run ahead of the actual
    /// DMA write-back. The only reliable signal is the descriptor's own DD
    /// (Descriptor Done) status bit, written back by hardware because every
    /// posted descriptor carries CMD.RS (Report Status). `e1000.rs` uses the
    /// same DD-bit check; this mirrors it instead of trusting TDH.
    fn can_send(&self) -> bool {
        dma_sync_rx_desc_span(
            &self.tx_ring,
            self.tx_ring_coherent,
            self.tx_tail,
            1,
            size_of::<TxDesc>(),
            DmaSyncDir::FromDevice,
        );
        let ring = self.tx_ring.as_ptr::<TxDesc>();
        let status = unsafe { read_volatile(&(*ring.add(self.tx_tail)).status) };
        status & TX_STAT_DD != 0
    }

    pub fn send(&mut self, data: &[u8]) -> DeviceResult {
        if data.is_empty() || data.len() > BUF_SIZE {
            return Err(DeviceError::InvalidParam);
        }

        // Check link via STATUS register
        if !self.link_up {
            let status = unsafe { mmio_read(self.base, E1000E_STATUS) };
            if status & STATUS_LU != 0 {
                self.link_up = true;
            } else {
                return Err(DeviceError::NotReady);
            }
        }

        if !self.can_send() {
            return Err(DeviceError::NotReady);
        }

        let idx = self.tx_tail;
        let ring = self.tx_ring.as_ptr::<TxDesc>();
        let desc = unsafe { &mut *ring.add(idx) };

        // Copy packet to TX buffer
        let buf = unsafe {
            core::slice::from_raw_parts_mut(self.tx_buf_vaddr(idx) as *mut u8, data.len())
        };
        buf.copy_from_slice(data);

        self.stats.tx_packets += 1;
        self.stats.tx_bytes += data.len() as u64;

        // Write descriptor fields (cmd last so HW doesn't fetch partial descriptor)
        unsafe {
            write_volatile(&mut desc.addr, self.tx_buf_paddr(idx));
            write_volatile(&mut desc.len, data.len() as u16);
            write_volatile(&mut desc.cso, 0);
            write_volatile(&mut desc.status, 0);
            write_volatile(&mut desc.css, 0);
            write_volatile(&mut desc.special, 0);
        }
        compiler_fence(Ordering::SeqCst);
        fence(Ordering::SeqCst);
        unsafe {
            write_volatile(&mut desc.cmd, TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS);
        }
        dma_sync_rx_desc_span(
            &self.tx_ring,
            self.tx_ring_coherent,
            idx,
            1,
            size_of::<TxDesc>(),
            DmaSyncDir::ToDevice,
        );
        dma_sync_region(
            &self.tx_buf_pool,
            self.tx_buf_coherent,
            idx * BUF_SIZE,
            data.len(),
            DmaSyncDir::ToDevice,
        );
        compiler_fence(Ordering::SeqCst);
        fence(Ordering::SeqCst);

        self.tx_tail = (idx + 1) % NUM_TX;
        unsafe {
            mmio_write(self.base, E1000E_TDT, self.tx_tail as u32);
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Watchdog — simple link check
    // -----------------------------------------------------------------------

    /// Returns `true` when carrier state changed.
    pub unsafe fn watchdog_tick(&mut self) -> bool {
        let now = timer_now_as_micros();
        let status = mmio_read(self.base, E1000E_STATUS);
        let link = status & STATUS_LU != 0;
        let mut link_changed = false;

        if link != self.link_up {
            link_changed = true;
            self.link_up = link;
            if link {
                crate::klog_warn!("[e1000e] link UP STATUS={:#010x}\n", status);
            } else {
                crate::klog_warn!("[e1000e] link DOWN\n");
            }
        }

        if link_changed || now >= self.watchdog_log_next_us {
            self.watchdog_log_next_us = now.saturating_add(E1000E_WATCHDOG_LOG_US);
            // GPRC>0 means the MAC received frames. MPC>0 means frames arrived but
            // were dropped (no free descriptors or DMA ring not armed).
            let gprc = mmio_read(self.base, E1000E_GPRC);
            let mpc = mmio_read(self.base, E1000E_MPC);
            // GPTC>0 means the MAC actually transmitted frames in this interval.
            // If a download stalls with GPRC stuck but GPTC still climbing, the
            // NIC is still sending our ACKs/window updates and the peer has gone
            // silent (TCP window / server side); if GPTC also stalls and TDH ==
            // TDT, our TX ring drained and we stopped queueing ACKs (TX side).
            let gptc = mmio_read(self.base, E1000E_GPTC);
            let tdh = mmio_read(self.base, E1000E_TDH);
            let tdt = mmio_read(self.base, E1000E_TDT);
            let csum_bad = self.rx_csum_bad;
            let tx_dropped = self.tx_dropped;
            crate::klog_info!(
                "[e1000e] watchdog: link={} GPRC={} MPC={} rx_pkt={} rx_csum_bad={} GPTC={} tx_pkt={} tx_drop={} TDH={} TDT={} itr={}\n",
                link,
                gprc,
                mpc,
                self.stats.rx_packets,
                csum_bad,
                gptc,
                self.stats.tx_packets,
                tx_dropped,
                tdh,
                tdt,
                self.itr_setting
            );
        }
        link_changed
    }

    fn program_itr(&mut self, itr: u32) {
        self.itr_setting = itr;
        unsafe { mmio_write(self.base, E1000E_ITR, itr) };
    }

    fn tune_itr(&mut self, now_us: u64, rx_event: bool) {
        if now_us < self.itr_tune_next_us && !rx_event {
            return;
        }
        self.itr_tune_next_us = now_us.saturating_add(E1000E_ITR_TUNE_PERIOD_US);
        let rx_now = self.stats.rx_packets;
        let rx_delta = rx_now.saturating_sub(self.itr_last_rx_packets);
        self.itr_last_rx_packets = rx_now;
        let target = if rx_event && rx_delta <= 4 {
            E1000E_ITR_LOW_LATENCY
        } else if rx_delta >= 64 {
            E1000E_ITR_THROUGHPUT
        } else {
            E1000E_ITR_BALANCED
        };
        if target != self.itr_setting {
            self.program_itr(target);
        }
    }

    fn merged_stats(&self) -> NetStats {
        self.stats.clone()
    }
}

// ---------------------------------------------------------------------------
// Eclipse OS driver wrappers
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct E1000eDriver {
    pub hw: Arc<Mutex<E1000eHw>>,
}

#[derive(Clone)]
pub struct E1000eInterface {
    pub iface: Arc<Mutex<Interface<'static, E1000eDriver>>>,
    pub driver: E1000eDriver,
    pub name: String,
    pub irq: usize,
    pub base: usize,
    pub poll_pending: Arc<AtomicBool>,
    /// Micros timestamp of the last time `poll_pending` was set true. Lets
    /// `heal_stuck_poll_pending` detect a bottom-half that was evicted from
    /// the deferred-job queue instead of one merely awaiting its turn.
    poll_pending_set_us: Arc<AtomicU64>,
    pub link_up_seen: Arc<AtomicBool>,
    watchdog_job_scheduled: Arc<AtomicBool>,
    pub routes: Arc<Mutex<Vec<RouteInfo>>>,
    pub ip_addrs: Arc<Mutex<Vec<IpCidr>>>,
}

impl E1000eInterface {
    pub fn schedule_watchdog(&self, fast: bool) {
        let now = timer_now_as_micros();
        {
            let mut hw = self.driver.hw.lock();
            if now < hw.link_watchdog_next_us && !fast {
                return;
            }
            if fast {
                hw.link_watchdog_next_us = now.saturating_add(E1000E_WATCHDOG_FAST_US);
            } else if now >= hw.link_watchdog_next_us {
                hw.link_watchdog_next_us = now.saturating_add(E1000E_WATCHDOG_PERIOD_US);
            }
        }
        if self.watchdog_job_scheduled.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone();
        crate::utils::deferred_job::push_deferred_job(move || {
            struct Guard(Arc<AtomicBool>);
            impl Drop for Guard {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::Release);
                }
            }
            let _g = Guard(Arc::clone(&me.watchdog_job_scheduled));
            me.watchdog_job_scheduled.store(false, Ordering::Release);
            let (link_changed, link_up) = {
                let mut hw = me.driver.hw.lock();
                let changed = unsafe { hw.watchdog_tick() };
                (changed, hw.link_up)
            };
            if link_changed {
                me.link_up_seen.store(link_up, Ordering::Release);
            }
            me.schedule_watchdog(false);
        });
    }

    /// If `poll_pending` has been stuck true far longer than any legitimate
    /// deferred-job latency, the IRQ bottom-half that owns clearing it (see
    /// `handle_irq`) was evicted from the shared deferred-job queue —
    /// `deferred_job.rs` caps it at 256 entries and silently drops the oldest
    /// without running it. Without this, every future interrupt would take
    /// `handle_irq`'s "already pending" fast path forever (re-arm and return)
    /// and `poll_with_irq_hint` would never run again except via periodic
    /// external polling (`poll_ifaces_throttled`). Called from `poll()`,
    /// which is reached by that periodic polling regardless of IRQ state, so
    /// this reliably self-heals even on an interface that stopped receiving
    /// interrupts entirely.
    fn heal_stuck_poll_pending(&self) {
        if !self.poll_pending.load(Ordering::SeqCst) {
            return;
        }
        let set_at = self.poll_pending_set_us.load(Ordering::Relaxed);
        let now = timer_now_as_micros();
        if set_at != 0 && now.saturating_sub(set_at) > POLL_PENDING_STUCK_US {
            crate::klog_warn!(
                "[e1000e] poll_pending stuck for >{}us, self-healing (deferred job likely evicted)\n",
                POLL_PENDING_STUCK_US
            );
            self.poll_pending.store(false, Ordering::SeqCst);
        }
    }

    fn ims_rearm(&self) {
        unsafe {
            compiler_fence(Ordering::SeqCst);
            mmio_write(self.base, E1000E_IMS, IMS_REARM);
            let _ = mmio_read(self.base, E1000E_IMS);
            fence(Ordering::SeqCst);
        }
    }

    /// NIC poll; `irq_icr` carries ICR bits when invoked from the deferred IRQ bottom-half.
    fn poll_with_irq_hint(&self, irq_icr: u32) -> DeviceResult {
        let now = timer_now_as_micros();
        let due = self.driver.hw.lock().link_watchdog_next_us <= now;
        if due {
            self.schedule_watchdog(false);
        }

        let ts = Instant::from_micros(now as i64);
        {
            let mut hw = self.driver.hw.lock();
            hw.handle_rx_irq(irq_icr);
        }
        unsafe {
            self.driver.hw.lock().ensure_rx_armed_if_link_up();
        }

        // Do NOT manually toggle interrupts around the SOCKETS + iface locks.
        // `super::intr_off/on` are raw hardware toggles (drivers_intr_*) that
        // bypass the kernel-sync Mutex's push_off/pop_off noff accounting. The
        // Mutex already keeps interrupts disabled for the whole locked critical
        // section, so a NIC IRQ cannot reenter and deadlock on SOCKETS while we
        // hold it. Force-toggling the raw flag here (as this code previously did,
        // mislabeled the "rtlx / e1000 pattern") desyncs pop_off and re-enables
        // interrupts mid-section under an outer lock holder — panicking
        // ("RefCell already borrowed") or deadlocking on SOCKETS under SMP.
        // e1000.rs and rtlx.rs document this exact hazard; rely on the Mutex alone.
        let sockets = get_sockets();
        let mut had_rx = (irq_icr & ICR_RX_ANY) != 0;
        // Acquire the socket set + iface with try_lock, exactly as the loopback
        // driver does. A blocking `lock()` here DEADLOCKS: this poll runs from a
        // deferred job that a socket syscall drains WHILE it holds the socket
        // set (e.g. a read/write draining the NIC to make progress), so on this
        // IRQ-off single-CPU kernel the acquire spins forever on a lock the same
        // CPU already owns — the >8s spinlock the red banner reported at this
        // line during a large `apk`/`wget` download. If either lock is held,
        // skip the smoltcp poll: the frame that owns the socket set drives it on
        // release, and `handle_rx_irq` above already serviced the RX-overrun bit
        // so nothing is lost.
        if let (Some(mut sockets), Some(mut iface)) = (sockets.try_lock(), self.iface.try_lock()) {
            match iface.poll(&mut sockets, ts) {
                Ok(true) => had_rx = true,
                Ok(false) => {}
                Err(e) => warn!("e1000e smoltcp poll: {:?}", e),
            }
        }

        super::net_flush_deferred_packets();
        {
            let mut hw = self.driver.hw.lock();
            // Ring the RDT doorbell once for however many packets `iface.poll`
            // just drained above, instead of once per packet inside the
            // receive loop — see `flush_rx_doorbell` for why this matters.
            hw.flush_rx_doorbell();
            hw.tune_itr(now, had_rx);
        }

        if had_rx {
            super::wake_net_rx_waiters();
        }
        Ok(())
    }
}

impl Scheme for E1000eInterface {
    fn name(&self) -> &str {
        "e1000e"
    }

    /// Minimal IRQ top-half (same as [`e1000::E1000Interface`]): read ICR, mask IMS,
    /// queue one deferred poll. RX waiters are woken from [`E1000eInterface::poll_with_irq_hint`]
    /// in thread context — never here (avoids `RefCell already borrowed`).
    fn handle_irq(&self, irq: usize) {
        if irq != self.irq {
            return;
        }

        let icr = unsafe { mmio_read(self.base, E1000E_ICR) };
        if icr == 0 {
            if !self.poll_pending.load(Ordering::SeqCst) {
                self.ims_rearm();
            }
            return;
        }

        if self.poll_pending.load(Ordering::SeqCst) {
            self.ims_rearm();
            return;
        }

        self.poll_pending.store(true, Ordering::SeqCst);
        self.poll_pending_set_us
            .store(timer_now_as_micros(), Ordering::Relaxed);
        unsafe {
            mmio_write(self.base, E1000E_IMC, 0xFFFF_FFFF);
            let _ = mmio_read(self.base, E1000E_IMC);
            fence(Ordering::SeqCst);
        }

        let poll_pending = self.poll_pending.clone();
        let me = self.clone();
        crate::utils::deferred_job::push_deferred_job(move || {
            if icr & ICR_LSC != 0 {
                me.schedule_watchdog(true);
            }
            let _ = me.poll_with_irq_hint(icr);
            // Clear poll_pending BEFORE re-arming IMS so that any IRQ that fires
            // after ims_rearm() finds poll_pending=false and properly queues a new
            // deferred job.  With IMS masked throughout the poll, new packets
            // accumulate in ICR; re-arming causes the NIC to re-assert the IRQ for
            // those accumulated bits.
            poll_pending.store(false, Ordering::SeqCst);
            me.ims_rearm();
        });
    }
}

impl NetScheme for E1000eInterface {
    fn get_mac(&self) -> EthernetAddress {
        self.iface.lock().ethernet_addr()
    }
    fn get_ifname(&self) -> String {
        self.name.clone()
    }
    fn get_ip_address(&self) -> Vec<IpCidr> {
        self.ip_addrs.lock().clone()
    }
    fn set_ipv4_address(&self, cidr: Ipv4Cidr) -> DeviceResult {
        let mut iface = self.iface.lock();
        iface.update_ip_addrs(|addrs| {
            let mut set_primary = false;
            for slot in addrs.iter_mut() {
                if let IpCidr::Ipv4(_) = slot {
                    if !set_primary {
                        *slot = IpCidr::Ipv4(cidr);
                        set_primary = true;
                    } else {
                        *slot = IpCidr::Ipv4(Ipv4Cidr::new(Ipv4Address::UNSPECIFIED, 0));
                    }
                }
            }
            if !set_primary {
                if let Some(slot) = addrs.iter_mut().next() {
                    *slot = IpCidr::Ipv4(cidr);
                }
            }
        });
        let addrs_vec = iface.ip_addrs().to_vec();
        *self.ip_addrs.lock() = addrs_vec;
        Ok(())
    }
    fn add_ip_address(&self, cidr: IpCidr) -> DeviceResult {
        let mut iface = self.iface.lock();
        iface.update_ip_addrs(|addrs| {
            if addrs.contains(&cidr) {
                return;
            }
            for slot in addrs.iter_mut() {
                if slot.address().is_unspecified() && slot.prefix_len() == 0 {
                    *slot = cidr;
                    return;
                }
            }
            if let Some(slot) = addrs.iter_mut().last() {
                *slot = cidr;
            }
        });
        *self.ip_addrs.lock() = iface.ip_addrs().to_vec();
        Ok(())
    }
    fn remove_ip_address(&self, cidr: IpCidr) -> DeviceResult {
        let mut iface = self.iface.lock();
        iface.update_ip_addrs(|addrs| {
            for slot in addrs.iter_mut() {
                if *slot == cidr {
                    *slot = IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0);
                    return;
                }
            }
        });
        *self.ip_addrs.lock() = iface.ip_addrs().to_vec();
        Ok(())
    }
    fn seed_neighbor(
        &self,
        protocol: smoltcp::wire::IpAddress,
        hardware: smoltcp::wire::EthernetAddress,
    ) -> DeviceResult {
        let ts = Instant::from_micros(timer_now_as_micros() as i64);
        self.iface.lock().seed_neighbor(protocol, hardware, ts);
        Ok(())
    }
    fn refresh_link(&self) -> DeviceResult {
        {
            let mut hw = self.driver.hw.lock();
            hw.link_up = false;
        }
        self.schedule_watchdog(true);
        Ok(())
    }
    fn link_carrier_up(&self) -> bool {
        self.driver.hw.lock().link_up
            || unsafe { mmio_read(self.base, E1000E_STATUS) & STATUS_LU != 0 }
    }
    fn poll(&self) -> DeviceResult {
        self.heal_stuck_poll_pending();
        self.poll_with_irq_hint(0)?;
        self.ims_rearm();
        Ok(())
    }
    fn recv(&self, buf: &mut [u8]) -> DeviceResult<usize> {
        let mut hw = self.driver.hw.lock();
        let pkt = hw.receive();
        // This path doesn't go through `poll_with_irq_hint` (which flushes
        // after draining smoltcp), so it must flush its own doorbell here.
        hw.flush_rx_doorbell();
        drop(hw);
        if let Some(pkt) = pkt {
            let n = pkt.len().min(buf.len());
            buf[..n].copy_from_slice(&pkt[..n]);
            Ok(n)
        } else {
            Err(DeviceError::NotReady)
        }
    }
    fn send(&self, data: &[u8]) -> DeviceResult<usize> {
        let mut hw = self.driver.hw.lock();
        hw.send(data)?;
        Ok(data.len())
    }
    fn can_recv(&self) -> bool {
        true
    }
    fn can_send(&self) -> bool {
        self.driver.hw.lock().can_send()
    }
    fn add_route(&self, cidr: IpCidr, gateway: Option<smoltcp::wire::IpAddress>) -> DeviceResult {
        let mut iface = self.iface.lock();
        match gateway {
            Some(IpAddress::Ipv4(gw)) => {
                if cidr.prefix_len() == 0 {
                    iface
                        .routes_mut()
                        .add_default_ipv4_route(gw)
                        .map_err(|_| DeviceError::IoError)?;
                }
                let mut routes = self.routes.lock();
                routes.retain(|r| !(matches!(r.dst, IpCidr::Ipv4(_)) && r.dst.prefix_len() == 0));
                routes.push(RouteInfo {
                    dst: cidr,
                    gateway: Some(IpAddress::Ipv4(gw)),
                });
            }
            Some(IpAddress::Ipv6(gw)) => {
                if cidr.prefix_len() == 0 {
                    iface
                        .routes_mut()
                        .add_default_ipv6_route(gw)
                        .map_err(|_| DeviceError::IoError)?;
                }
                let mut routes = self.routes.lock();
                routes.retain(|r| !(matches!(r.dst, IpCidr::Ipv6(_)) && r.dst.prefix_len() == 0));
                routes.push(RouteInfo {
                    dst: cidr,
                    gateway: Some(IpAddress::Ipv6(gw)),
                });
            }
            None => {
                self.routes.lock().push(RouteInfo { dst: cidr, gateway });
            }
            _ => {}
        }
        Ok(())
    }
    fn del_route(&self, cidr: IpCidr, _gateway: Option<smoltcp::wire::IpAddress>) -> DeviceResult {
        let mut iface = self.iface.lock();
        if cidr.prefix_len() == 0 {
            match cidr {
                IpCidr::Ipv4(_) => {
                    let _ = iface.routes_mut().remove_default_ipv4_route();
                }
                IpCidr::Ipv6(_) => {
                    let _ = iface.routes_mut().remove_default_ipv6_route();
                }
                _ => {}
            }
        }
        self.routes.lock().retain(|r| r.dst != cidr);
        Ok(())
    }
    fn get_routes(&self) -> Vec<RouteInfo> {
        let iface = self.iface.lock();
        let mut res = self.routes.lock().clone();
        for cidr in iface.ip_addrs() {
            match cidr {
                IpCidr::Ipv4(v4) if v4.prefix_len() > 0 => {
                    res.push(RouteInfo {
                        dst: IpCidr::Ipv4(v4.network()),
                        gateway: None,
                    });
                }
                IpCidr::Ipv6(v6) if v6.prefix_len() > 0 => {
                    res.push(RouteInfo {
                        dst: IpCidr::Ipv6(v6.network()),
                        gateway: None,
                    });
                }
                _ => {}
            }
        }
        res
    }
    fn get_stats(&self) -> NetStats {
        self.driver.hw.lock().merged_stats()
    }
    fn get_mtu(&self) -> usize {
        1500
    }
}

// ---------------------------------------------------------------------------
// smoltcp Device impl
// ---------------------------------------------------------------------------

pub struct E1000eRxToken {
    data: Vec<u8>,
}
pub struct E1000eTxToken(E1000eDriver);

impl phy::Device<'_> for E1000eDriver {
    type RxToken = E1000eRxToken;
    type TxToken = E1000eTxToken;

    fn receive(&mut self) -> Option<(Self::RxToken, Self::TxToken)> {
        let mut hw = self.hw.lock();
        hw.receive()
            .map(|pkt| (E1000eRxToken { data: pkt }, E1000eTxToken(self.clone())))
    }
    fn transmit(&mut self) -> Option<Self::TxToken> {
        if self.hw.lock().can_send() {
            Some(E1000eTxToken(self.clone()))
        } else {
            None
        }
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1514;
        caps.max_burst_size = Some(64);
        caps
    }
}

impl phy::RxToken for E1000eRxToken {
    fn consume<R, F>(self, _ts: Instant, f: F) -> SmolResult<R>
    where
        F: FnOnce(&mut [u8]) -> SmolResult<R>,
    {
        let mut data = self.data;
        super::net_defer_packet(&data);
        f(&mut data)
    }
}

impl phy::TxToken for E1000eTxToken {
    fn consume<R, F>(self, _ts: Instant, len: usize, f: F) -> SmolResult<R>
    where
        F: FnOnce(&mut [u8]) -> SmolResult<R>,
    {
        let len = len.min(65536);
        let mut buf = vec![0u8; len];
        let result = f(&mut buf)?;
        let mut hw = self.0.hw.lock();
        // NEVER silently drop a frame smoltcp handed us. The ingress path
        // (`socket_ingress`) dispatches the ACK / window-update generated in
        // direct response to a received segment through the TxToken paired with
        // the RxToken — and if that send fails, smoltcp only logs it and moves
        // on, having ALREADY advanced its remote_last_ack/remote_last_win state.
        // The dropped ACK is never re-emitted, so the peer keeps waiting on a
        // window it thinks is closed and the whole transfer deadlocks. On real
        // hardware the 256-deep TX ring transiently fills under an RX burst
        // (ITR throttles TX completion), losing exactly these ACKs — the silent
        // mid-download stall seen only on real hardware, never under QEMU (which
        // completes TX synchronously, so the ring never fills). The NIC drains
        // the ring autonomously via DMA, so spin briefly on a free slot instead
        // of dropping.
        let mut tries = 0;
        loop {
            match hw.send(&buf) {
                Ok(()) => return Ok(result),
                Err(_) if tries < TX_SEND_SPIN_LIMIT => {
                    tries += 1;
                    core::hint::spin_loop();
                }
                Err(_) => {
                    hw.tx_dropped += 1;
                    return Err(smoltcp::Error::Exhausted);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: ensure RX ring is armed when link comes up
// ---------------------------------------------------------------------------

impl E1000eHw {
    pub unsafe fn ensure_rx_armed_if_link_up(&mut self) {
        // Once link is already known up, this can only ever set link_up=true
        // again — a no-op — so skip the MMIO STATUS read. This runs on every
        // single poll (IRQ-driven or periodic), so during steady-state
        // traffic (the common case) it was an unconditional register read
        // that accomplished nothing. Link-DOWN transitions are still caught
        // by `watchdog_tick`'s own STATUS read on its normal cadence.
        if self.link_up {
            return;
        }
        let status = mmio_read(self.base, E1000E_STATUS);
        if status & STATUS_LU != 0 {
            self.link_up = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Public init — called from pci.rs
// ---------------------------------------------------------------------------

pub fn init(
    name: String,
    pci: &PCIDevice,
    irq: usize,
    vaddr: usize,
    _index: usize,
) -> DeviceResult<E1000eInterface> {
    crate::klog_warn!(
        "[e1000e] probing {} vaddr={:#x} irq={} device={:#x} tag={}\n",
        name,
        vaddr,
        irq,
        pci.id.device_id,
        E1000E_DRIVER_TAG
    );

    // Split the DMA memory by access pattern, fixing both the UC-slowness AND
    // the WB false-sharing failure modes (QEMU, coherent with no real cache,
    // never exposes either):
    //
    // Descriptor RINGS -> uncached (try_coherent). A descriptor is 16 bytes, so
    // four pack into one 64-byte cache line. With WB+clflush, recycling one
    // descriptor dirties its line and the ToDevice clflush writes the WHOLE line
    // back — including neighbours the NIC may have just stamped DD into (in RAM)
    // a moment earlier. That stale write-back erases their DD bits, the driver
    // never sees those completions, and RX wedges: a deterministic mid-transfer
    // stall on real hardware (the race cannot happen under QEMU's synchronous
    // device model). UC sidesteps it entirely — and the rings are tiny, so the
    // uncached cost is negligible.
    //
    // Packet BUFFERS -> write-back + clflush (alloc_uninit). These carry the
    // bulk RX/TX payload, where uncached reads are ~100x slower and starve the
    // RX drain on a multi-MB stream. They are safe as WB: BUF_SIZE is a multiple
    // of the cache line and the pool is page-aligned, so buffers never share a
    // line (no false sharing), RX buffers are only read by the CPU (FromDevice
    // clflush only invalidates, never writes a stale line back), and
    // alloc_uninit now evicts each region's lines at allocation so it starts
    // clean.
    let (rx_ring, rx_ring_coherent) =
        DmaRegion::alloc_uninit_try_coherent(NUM_RX * size_of::<RxDesc>())
            .ok_or(DeviceError::DmaError)?;
    let (tx_ring, tx_ring_coherent) =
        DmaRegion::alloc_uninit_try_coherent(NUM_TX * size_of::<TxDesc>())
            .ok_or(DeviceError::DmaError)?;
    let rx_buf_pool = DmaRegion::alloc_uninit(NUM_RX * BUF_SIZE).ok_or(DeviceError::DmaError)?;
    let tx_buf_pool = DmaRegion::alloc_uninit(NUM_TX * BUF_SIZE).ok_or(DeviceError::DmaError)?;
    let rx_buf_coherent = false;
    let tx_buf_coherent = false;
    crate::klog_warn!(
        "[e1000e] LK-RX path: rings UC(rx={} tx={}), buffers write-back+clflush\n",
        rx_ring_coherent,
        tx_ring_coherent
    );

    // Alignment checks
    for (label, region, align, span) in [
        ("rx_ring", &rx_ring, DMA_DESC_ALIGN, DMA_RING_BYTES),
        ("tx_ring", &tx_ring, DMA_DESC_ALIGN, DMA_TX_RING_BYTES),
        ("rx_buf_pool", &rx_buf_pool, 64, NUM_RX * BUF_SIZE),
        ("tx_buf_pool", &tx_buf_pool, 64, NUM_TX * BUF_SIZE),
    ] {
        if region.paddr() % align != 0 || region.vaddr() % align != 0 {
            crate::klog_err!("[e1000e] {} DMA misaligned\n", label);
            return Err(DeviceError::DmaError);
        }
        if region.byte_len() < span {
            crate::klog_err!("[e1000e] {} too small\n", label);
            return Err(DeviceError::DmaError);
        }
    }

    let mut hw = E1000eHw {
        base: vaddr,
        pci_loc: pci.loc,
        device_id: pci.id.device_id,
        mac: [0u8; 6],
        rx_ring,
        rx_buf_pool,
        rx_ring_coherent,
        rx_buf_coherent,
        rx_next_to_clean: 0,
        rx_pending: None,
        rx_doorbell_dirty: false,
        tx_ring,
        tx_buf_pool,
        tx_ring_coherent,
        tx_buf_coherent,
        tx_tail: 0,
        stats: NetStats::default(),
        rx_csum_bad: 0,
        tx_dropped: 0,
        link_up: false,
        link_watchdog_next_us: 0,
        watchdog_log_next_us: 0,
        itr_setting: E1000E_ITR_BALANCED,
        itr_last_rx_packets: 0,
        itr_tune_next_us: 0,
    };

    unsafe {
        hw.reset_and_init()?;
    }

    let mac_bytes = hw.mac;
    crate::klog_warn!(
        "e1000e: {} {:#x}:{:#x} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} tag={}\n",
        name,
        pci.id.vendor_id,
        pci.id.device_id,
        mac_bytes[0],
        mac_bytes[1],
        mac_bytes[2],
        mac_bytes[3],
        mac_bytes[4],
        mac_bytes[5],
        E1000E_DRIVER_TAG
    );

    let hw_arc = Arc::new(Mutex::new(hw));
    let driver = E1000eDriver { hw: hw_arc.clone() };

    let ethernet_addr = EthernetAddress::from_bytes(&mac_bytes);

    // IPv6 link-local from EUI-64
    let mut eui64 = [0u8; 8];
    eui64[0] = mac_bytes[0] ^ 2;
    eui64[1] = mac_bytes[1];
    eui64[2] = mac_bytes[2];
    eui64[3] = 0xff;
    eui64[4] = 0xfe;
    eui64[5] = mac_bytes[3];
    eui64[6] = mac_bytes[4];
    eui64[7] = mac_bytes[5];
    let link_local = Ipv6Address::new(
        0xfe80,
        0,
        0,
        0,
        (eui64[0] as u16) << 8 | eui64[1] as u16,
        (eui64[2] as u16) << 8 | eui64[3] as u16,
        (eui64[4] as u16) << 8 | eui64[5] as u16,
        (eui64[6] as u16) << 8 | eui64[7] as u16,
    );

    let ip_addrs = vec![
        IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0),
        IpCidr::Ipv6(Ipv6Cidr::new(link_local, 64)),
        IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0),
        IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0),
    ];
    let default_v4_gw = Ipv4Address::new(0, 0, 0, 0);
    // Fresh per-NIC storage, NOT a shared `static mut`: a shared static would
    // hand out a second live `&'static mut` on every additional e1000e probed
    // (aliasing UB) and would splice every NIC's routes into the same 4-entry
    // table. Leaking is fine here — this runs once per NIC for the life of the
    // kernel, same as the mock test harness's `Box::leak` for its register file.
    let routes_storage: &'static mut [Option<(IpCidr, Route)>] =
        Box::leak(vec![None; 4].into_boxed_slice());
    let mut routes = Routes::new(routes_storage);
    routes.add_default_ipv4_route(default_v4_gw).unwrap();
    let neighbor_cache = NeighborCache::new(BTreeMap::new());

    let iface = InterfaceBuilder::new(driver.clone())
        .ethernet_addr(ethernet_addr)
        .neighbor_cache(neighbor_cache)
        .ip_addrs(ip_addrs.clone())
        .routes(routes)
        .finalize();

    let link_up_seen = Arc::new(AtomicBool::new(unsafe {
        mmio_read(vaddr, E1000E_STATUS) & STATUS_LU != 0
    }));
    let e1000e_iface = E1000eInterface {
        iface: Arc::new(Mutex::new(iface)),
        driver,
        name,
        irq,
        base: vaddr,
        poll_pending: Arc::new(AtomicBool::new(false)),
        poll_pending_set_us: Arc::new(AtomicU64::new(0)),
        link_up_seen,
        watchdog_job_scheduled: Arc::new(AtomicBool::new(false)),
        routes: Arc::new(Mutex::new(vec![RouteInfo {
            dst: IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0),
            gateway: Some(IpAddress::Ipv4(default_v4_gw)),
        }])),
        ip_addrs: Arc::new(Mutex::new(ip_addrs)),
    };

    Ok(e1000e_iface)
}

// ---------------------------------------------------------------------------
// PCI driver registration
// ---------------------------------------------------------------------------

pub struct E1000eDriverPci;

impl PciDriver for E1000eDriverPci {
    fn name(&self) -> &str {
        "e1000e"
    }

    fn matched(&self, vendor_id: u16, device_id: u16) -> bool {
        if vendor_id != 0x8086 {
            return false;
        }
        matches!(
            device_id,
            // NOTE: 0x1533 (I210), 0x1539 (I211) and 0x157b/0x157c (I210
            // flashless) are deliberately NOT matched here — they are igb-family
            // silicon (Linux drives them with `igb`, not `e1000e`) and this
            // driver's is_pch()/is_pch_spt_or_later() gates exclude them, so
            // RXDCTL.QUEUE_ENABLE is never programmed for them: matching those
            // IDs bound the interface but left it permanently unable to RX/TX
            // with no error reported.
            0x10d3 | 0x10f5 | 0x150c |
            0x1502..=0x1503 | 0x153a..=0x153b | 0x155a | 0x1559 |
            0x15a0..=0x15a3 | 0x156f..=0x1570 | 0x15b7..=0x15be |
            0x15d6..=0x15d8 | 0x15e3 |
            0x0d4c..=0x0d4f | 0x15f4..=0x15fc | 0x1a1c..=0x1a1f |
            0x0dc5..=0x0dc8 | 0x550a..=0x5511 | 0x57a0..=0x57a1 |
            0x57b3..=0x57ba | 0x15df..=0x15e2 | 0x0d53 | 0x0d55
        )
    }

    fn init(
        &self,
        dev: &PCIDevice,
        mapper: &Option<Arc<dyn IoMapper>>,
        irq: Option<usize>,
    ) -> DeviceResult<Device> {
        crate::klog_warn!(
            "e1000e: probe PCI {:#x}:{:#x} tag={}\n",
            dev.id.vendor_id,
            dev.id.device_id,
            E1000E_DRIVER_TAG
        );
        let bar0_addr = if let Some(BAR::Memory(a, _, _, _)) = dev.bars[0] {
            a as usize
        } else {
            return Err(DeviceError::IoError);
        };

        if let Some(m) = mapper {
            m.query_or_map(bar0_addr, 128 * 1024);
        }

        let vaddr = crate::net::phys_to_virt(bar0_addr);
        // Naming by bus alone collides whenever a second NIC (or a second
        // function of a multi-port card) sits on the same PCI bus at a
        // different device/function — two interfaces would both be "ethN".
        // Keep the common single-function-per-bus case as the familiar
        // "ethN"; disambiguate only when device/function actually vary, which
        // is guaranteed unique since (bus, device, function) is the PCI
        // addressing key.
        let name = if dev.loc.device == 0 && dev.loc.function == 0 {
            alloc::format!("eth{}", dev.loc.bus)
        } else {
            alloc::format!("eth{}_{}_{}", dev.loc.bus, dev.loc.device, dev.loc.function)
        };

        unsafe {
            let mut cmd = PCI_ACCESS.read16(&PortOpsImpl, dev.loc, 0x04);
            cmd |= 0x0004 | 0x0002;
            PCI_ACCESS.write16(&PortOpsImpl, dev.loc, 0x04, cmd);
        }

        let vector = irq.map(|idx| idx + 32).unwrap_or(0);
        let iface = init(name, dev, vector, vaddr, 0)?;
        let iface_arc = Arc::new(iface);
        iface_arc.schedule_watchdog(true);
        if vector != 0 {
            crate::net::pci_note_pending_msi(vector, iface_arc.clone());
        }
        Ok(Device::Net(iface_arc))
    }
}

#[cfg(test)]
mod rx_ring_tests {
    //! Host bench for the e1000e RX descriptor-ring state machine.
    //!
    //! Drives the *real* `receive`/`process_rx_slot`/`recycle_rx_slot` against a
    //! simulated NIC: host memory stands in for the descriptor ring, the packet
    //! buffers and the MMIO register file (RDH/RDT), with phys==virt identity
    //! mapping. We play the hardware (fill descriptors, advance RDH) and assert
    //! the driver hands every frame back intact, recycles slots, advances RDT,
    //! and keeps working across a full-ring fill and ring wrap-around — the
    //! conditions a large download exercises and where a wedge would hide.

    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use std::alloc::{alloc_zeroed, Layout};

    // --- mock kernel hooks: identity-mapped host memory, no-op the rest ---
    #[no_mangle]
    extern "C" fn drivers_dma_alloc(pages: usize) -> usize {
        let layout = Layout::from_size_align(pages * 4096, 4096).unwrap();
        unsafe { alloc_zeroed(layout) as usize }
    }
    #[no_mangle]
    extern "C" fn drivers_dma_dealloc(_p: usize, _pages: usize) -> i32 {
        0
    }
    #[no_mangle]
    extern "C" fn drivers_phys_to_virt(p: usize) -> usize {
        p
    }
    #[no_mangle]
    extern "C" fn drivers_virt_to_phys(v: usize) -> usize {
        v
    }
    #[no_mangle]
    extern "C" fn drivers_dma_mark_uncached(_p: usize, _pages: usize) -> i32 {
        0
    }
    #[no_mangle]
    extern "C" fn drivers_dma_verify_uncached(_p: usize, _pages: usize) -> i32 {
        0
    }
    #[no_mangle]
    extern "C" fn drivers_timer_now_as_micros() -> u64 {
        0
    }
    #[no_mangle]
    extern "C" fn drivers_klog_emit(_priority: u8, _msg: *const u8, _len: usize) {}
    #[no_mangle]
    extern "C" fn drivers_intr_on() {}
    #[no_mangle]
    extern "C" fn drivers_intr_off() {}
    #[no_mangle]
    extern "C" fn drivers_intr_get() -> bool {
        false
    }
    #[no_mangle]
    extern "C" fn drivers_wake_net_rx_waiters() {}
    #[no_mangle]
    extern "C" fn drivers_net_drain() {}

    fn reg_read(base: usize, reg: usize) -> u32 {
        unsafe { core::ptr::read_volatile((base + reg * 4) as *const u32) }
    }
    fn reg_write(base: usize, reg: usize, val: u32) {
        unsafe { core::ptr::write_volatile((base + reg * 4) as *mut u32, val) };
    }

    /// Build an `E1000eHw` over host memory, with the RX ring initialized exactly
    /// like `init_rx` (each descriptor points at its buffer, RDH=0, RDT=NUM_RX-1).
    pub(super) fn make_hw() -> E1000eHw {
        let regs = Box::leak(vec![0u32; 0x4000].into_boxed_slice());
        let base = regs.as_ptr() as usize;
        let rx_ring = DmaRegion::alloc(NUM_RX * core::mem::size_of::<RxDesc>()).unwrap();
        let rx_buf_pool = DmaRegion::alloc_uninit(NUM_RX * BUF_SIZE).unwrap();
        let tx_ring = DmaRegion::alloc(NUM_TX * core::mem::size_of::<TxDesc>()).unwrap();
        let tx_buf_pool = DmaRegion::alloc(NUM_TX * BUF_SIZE).unwrap();

        let mut hw = E1000eHw {
            base,
            pci_loc: Location {
                bus: 0,
                device: 0,
                function: 0,
            },
            device_id: 0x10d3,
            mac: [0x52, 0x54, 0, 0, 0, 1],
            rx_ring,
            rx_buf_pool,
            rx_ring_coherent: false,
            rx_buf_coherent: false,
            rx_next_to_clean: 0,
            rx_pending: None,
            rx_doorbell_dirty: false,
            tx_ring,
            tx_buf_pool,
            tx_ring_coherent: false,
            tx_buf_coherent: false,
            tx_tail: 0,
            stats: NetStats::default(),
            rx_csum_bad: 0,
            tx_dropped: 0,
            link_up: true,
            link_watchdog_next_us: 0,
            watchdog_log_next_us: 0,
            itr_setting: 0,
            itr_last_rx_packets: 0,
            itr_tune_next_us: 0,
        };
        // Initialize the descriptor ring (mirror of init_rx).
        let ring = hw.rx_ring.as_ptr::<RxDesc>();
        for i in 0..NUM_RX {
            unsafe {
                let d = &mut *ring.add(i);
                d.addr = hw.rx_buf_paddr(i);
                d.len = 0;
                d.chksum = 0;
                d.status = 0;
                d.errors = 0;
                d.vlan = 0;
            }
        }
        reg_write(base, E1000E_RDH, 0);
        reg_write(base, E1000E_RDT, (NUM_RX - 1) as u32);

        // Initialize the TX descriptor ring (mirror of init_tx): every slot
        // starts DD (done/free) so `can_send`/`send` can post from slot 0.
        let tx_ring_ptr = hw.tx_ring.as_ptr::<TxDesc>();
        for i in 0..NUM_TX {
            unsafe {
                let d = &mut *tx_ring_ptr.add(i);
                d.addr = hw.tx_buf_paddr(i);
                d.len = 0;
                d.cso = 0;
                d.cmd = 0;
                d.status = TX_STAT_DD;
                d.css = 0;
                d.special = 0;
            }
        }
        reg_write(base, E1000E_TDH, 0);
        reg_write(base, E1000E_TDT, 0);
        hw
    }

    /// Play the hardware: DMA `data` into slot `slot`, mark it DD|EOP, advance RDH.
    fn hw_deliver(hw: &E1000eHw, slot: usize, data: &[u8]) {
        assert!(data.len() <= BUF_SIZE);
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                hw.rx_buf_vaddr(slot) as *mut u8,
                data.len(),
            );
            let d = &mut *hw.rx_ring.as_ptr::<RxDesc>().add(slot);
            d.addr = hw.rx_buf_paddr(slot);
            d.len = data.len() as u16;
            d.status = RXD_STAT_DD | RXD_STAT_EOP;
            d.errors = 0;
        }
        // HW head now points one past the slot it just filled.
        reg_write(hw.base, E1000E_RDH, ((slot + 1) % NUM_RX) as u32);
    }

    fn pkt(seed: u8, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| seed.wrapping_add(i as u8).wrapping_mul(31))
            .collect()
    }

    #[test]
    fn rx_single_packet_roundtrips() {
        let mut hw = make_hw();
        let p = pkt(7, 512);
        hw_deliver(&hw, 0, &p);
        let got = hw.receive().expect("expected a frame");
        assert_eq!(got, p, "frame payload mismatch");
        assert_eq!(hw.stats.rx_packets, 1);
        // The RDT doorbell is deferred (batched across a receive burst, see
        // `flush_rx_doorbell`) rather than rung on every single packet, so
        // the caller must flush explicitly before the slot is handed back to
        // HW via RDT.
        hw.flush_rx_doorbell();
        assert_eq!(
            reg_read(hw.base, E1000E_RDT) as usize,
            0,
            "RDT should point at recycled slot 0"
        );
    }

    #[test]
    fn rx_doorbell_is_batched_not_rung_per_packet() {
        let mut hw = make_hw();
        // Deliver several packets up front, draining each one via receive()
        // WITHOUT flushing in between — mirroring how a real poll burst
        // drains many packets before the caller flushes once at the end.
        for i in 0..5usize {
            hw_deliver(&hw, i, &pkt(i as u8, 64));
        }
        for i in 0..5usize {
            let got = hw
                .receive()
                .unwrap_or_else(|| panic!("missing frame {}", i));
            assert_eq!(got, pkt(i as u8, 64), "frame {i} mismatch");
            // The doorbell must NOT move until flush_rx_doorbell is called —
            // recycling a slot only marks it dirty, it doesn't ring RDT.
            assert_eq!(
                reg_read(hw.base, E1000E_RDT) as usize,
                NUM_RX - 1,
                "RDT must stay at its initial value until flushed (packet {i})"
            );
        }
        // One flush must catch up the whole batch in a single write, jumping
        // straight to the last recycled slot instead of replaying each one.
        hw.flush_rx_doorbell();
        assert_eq!(
            reg_read(hw.base, E1000E_RDT) as usize,
            4,
            "flush should jump RDT straight to the last recycled slot"
        );
        // A second flush with nothing new recycled since must be a no-op.
        hw.flush_rx_doorbell();
        assert_eq!(reg_read(hw.base, E1000E_RDT) as usize, 4);
    }

    #[test]
    fn rx_burst_in_order() {
        let mut hw = make_hw();
        let n = 200usize;
        for i in 0..n {
            hw_deliver(&hw, i, &pkt(i as u8, 64 + i % 900));
        }
        for i in 0..n {
            let got = hw
                .receive()
                .unwrap_or_else(|| panic!("missing frame {}", i));
            assert_eq!(got, pkt(i as u8, 64 + i % 900), "frame {i} mismatch");
        }
        assert!(hw.receive().is_none(), "no more frames expected");
        assert_eq!(hw.stats.rx_packets, n as u64);
    }

    #[test]
    fn rx_full_ring_then_recover() {
        let mut hw = make_hw();
        // Fill every usable slot (HW leaves one guard between RDH and RDT).
        let fill = NUM_RX - 1;
        for i in 0..fill {
            hw_deliver(&hw, i, &pkt(i as u8, 128));
        }
        let mut drained = 0;
        while let Some(got) = hw.receive() {
            assert_eq!(got, pkt(drained as u8, 128), "frame {drained} mismatch");
            drained += 1;
        }
        assert_eq!(drained, fill, "should drain the whole ring");
        // After draining, RDT must have advanced so HW can use slots again:
        // deliver one more past the wrap and confirm it is received.
        let next = fill % NUM_RX;
        hw_deliver(&hw, next, &pkt(0xAB, 256));
        let got = hw.receive().expect("ring did not recover after full drain");
        assert_eq!(got, pkt(0xAB, 256));
    }

    #[test]
    fn rx_wraps_around_ring() {
        let mut hw = make_hw();
        // Push well over NUM_RX packets, draining as we go, to wrap rx_next_to_clean.
        let total = NUM_RX * 3 + 17;
        let mut produced = 0usize;
        let mut consumed = 0usize;
        while consumed < total {
            // keep the ring partly filled
            while produced < total && (produced - consumed) < NUM_RX - 1 {
                hw_deliver(&hw, produced % NUM_RX, &pkt(produced as u8, 100));
                produced += 1;
            }
            while let Some(got) = hw.receive() {
                assert_eq!(
                    got,
                    pkt(consumed as u8, 100),
                    "wrap frame {consumed} mismatch"
                );
                consumed += 1;
            }
        }
        assert_eq!(consumed, total);
        assert_eq!(hw.stats.rx_packets, total as u64);
    }
}

#[cfg(test)]
mod tx_ring_tests {
    //! Host bench for the e1000e TX descriptor-ring DD-bit ownership state
    //! machine (`can_send`/`send`), covering the fix for using a live TDH
    //! MMIO read as the completion signal (which Intel documents as
    //! unreliable — TDH reflects prefetch, not write-back) instead of the
    //! descriptor's own DD status bit that hardware sets on completion
    //! because every posted descriptor carries CMD.RS.

    use super::rx_ring_tests::make_hw;
    use alloc::vec::Vec;

    fn reg_read(base: usize, reg: usize) -> u32 {
        unsafe { core::ptr::read_volatile((base + reg * 4) as *const u32) }
    }

    fn pkt(seed: u8, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| seed.wrapping_add(i as u8).wrapping_mul(31))
            .collect()
    }

    /// Play the hardware completing a TX descriptor: write DD back into its
    /// status byte, exactly like a real NIC's write-back after CMD.RS.
    fn hw_complete_tx(hw: &super::E1000eHw, slot: usize) {
        unsafe {
            let d = &mut *hw.tx_ring.as_ptr::<super::TxDesc>().add(slot);
            d.status = super::TX_STAT_DD;
        }
    }

    #[test]
    fn tx_starts_free_and_posts_from_slot_zero() {
        let mut hw = make_hw();
        assert!(hw.can_send(), "every descriptor starts DD (free)");
        hw.send(&pkt(1, 64)).expect("first send must succeed");
        assert_eq!(reg_read(hw.base, super::E1000E_TDT), 1, "TDT advanced");
    }

    #[test]
    fn tx_slot_stays_owned_by_hw_until_dd_write_back() {
        let mut hw = make_hw();
        // Fill a full lap so tx_tail wraps back around to slot 0 — the very
        // descriptor the first send() below posts into.
        for i in 0..super::NUM_TX {
            hw.send(&pkt(i as u8, 64))
                .unwrap_or_else(|_| panic!("slot {} should be free on the first lap", i));
        }
        // tx_tail is back at slot 0. Hardware has not completed its DMA yet:
        // the descriptor's DD bit is still clear (send() clears it before
        // posting), so reusing it must be refused — a stale TDH read would
        // have let this through even though the NIC might still be reading
        // the buffer.
        assert!(
            !hw.can_send(),
            "slot 0 must stay owned by hardware until DD is written back"
        );
        assert!(hw.send(&pkt(0xFF, 64)).is_err());

        // Hardware finishes the DMA and writes DD back.
        hw_complete_tx(&hw, 0);
        assert!(hw.can_send(), "DD write-back must free the slot");
        hw.send(&pkt(0xEE, 64)).expect("slot reusable after DD");
    }

    #[test]
    fn tx_fills_the_full_ring_with_no_guard_slot() {
        // Unlike the old TDH-arithmetic scheme, DD-bit ownership is decided
        // per-slot, so there is no head==tail ambiguity requiring a reserved
        // guard descriptor — the whole ring (NUM_TX, not NUM_TX - 1) can be
        // posted before hardware needs to complete anything.
        let mut hw = make_hw();
        for i in 0..super::NUM_TX {
            hw.send(&pkt(i as u8, 32))
                .unwrap_or_else(|_| panic!("slot {} should be free", i));
        }
        assert!(!hw.can_send(), "ring is fully posted, no free slots left");
        assert!(hw.send(&pkt(0xAA, 32)).is_err());

        // Complete slot 0 (the oldest, and the next one `tx_tail` wraps to)
        // and confirm the ring recovers.
        hw_complete_tx(&hw, 0);
        assert!(hw.can_send(), "completing the oldest slot frees it up");
        hw.send(&pkt(0xBB, 32)).expect("ring recovers after completion");
    }

    #[test]
    fn tx_wraps_around_ring_interleaved_with_completions() {
        let mut hw = make_hw();
        let total = super::NUM_TX * 3 + 11;
        for i in 0..total {
            let slot = i % super::NUM_TX;
            // Simulate the NIC keeping up: complete each slot right after
            // it's posted, before it comes back around the ring.
            if i >= super::NUM_TX {
                hw_complete_tx(&hw, slot);
            }
            hw.send(&pkt(i as u8, 40))
                .unwrap_or_else(|_| panic!("send {} (slot {}) should succeed", i, slot));
        }
        assert_eq!(hw.stats.tx_packets, total as u64);
    }
}

#[cfg(test)]
mod coherency_bench {
    //! Host reproduction of the real-hardware-only RX wedge on a large download.
    //!
    //! A coherent host (or QEMU) cannot expose a DMA cache bug, so we model it:
    //! `ram` is what the NIC sees (it DMAs straight to RAM), and the driver
    //! accesses the descriptor ring through a simulated write-back cache. The
    //! NIC and the driver run interleaved — crucially the NIC fills the *next*
    //! descriptor while the driver is mid-processing the current one (real
    //! concurrent DMA), which QEMU's synchronous model never does.
    //!
    //! With write-back descriptor rings this reproduces the deterministic stall:
    //! a 16-byte descriptor shares a 64-byte cache line with three neighbours,
    //! so recycling one and flushing its line writes the stale neighbours back
    //! over DD bits the NIC just set -> lost completion -> RX wedges partway
    //! through a >100 MB stream. With uncached rings (the fix) the whole stream
    //! is delivered. The bench fails until the driver's ring is coherent.

    use alloc::collections::BTreeMap;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    const N: usize = 256; // ring slots
    const PL: usize = 4; // 16-byte descriptors per 64-byte cache line

    #[derive(Clone, Copy, Default, PartialEq)]
    struct D {
        dd: bool,
        seq: u32,
    }

    struct Model {
        ram: Vec<D>,                             // NIC's view (DMA target)
        cache: BTreeMap<usize, ([D; PL], bool)>, // driver WB cache: line -> (data, dirty)
        uc: bool,                                // true => uncached ring (no cache)
        rdh: usize,                              // NIC head
    }
    impl Model {
        fn line(i: usize) -> usize {
            i / PL
        }
        fn load_line(&mut self, l: usize) {
            if !self.cache.contains_key(&l) {
                let mut a = [D::default(); PL];
                for k in 0..PL {
                    a[k] = self.ram[l * PL + k];
                }
                self.cache.insert(l, (a, false));
            }
        }
        // NIC DMAs a completed descriptor straight into RAM, advancing head.
        fn nic_fill(&mut self, seq: u32) {
            let s = self.rdh;
            self.ram[s] = D { dd: true, seq };
            self.rdh = (self.rdh + 1) % N;
        }
        fn clflush_inval(&mut self, i: usize) {
            if !self.uc {
                self.cache.remove(&Self::line(i));
            }
        }
        fn read(&mut self, i: usize) -> D {
            if self.uc {
                return self.ram[i];
            }
            let l = Self::line(i);
            self.load_line(l);
            self.cache.get(&l).unwrap().0[i % PL]
        }
        fn write(&mut self, i: usize, d: D) {
            if self.uc {
                self.ram[i] = d;
                return;
            }
            let l = Self::line(i);
            self.load_line(l);
            let e = self.cache.get_mut(&l).unwrap();
            e.0[i % PL] = d;
            e.1 = true;
        }
        // ToDevice flush: write the WHOLE 64-byte line back (this is the bug:
        // stale neighbours clobber DD bits the NIC set after the line loaded).
        fn clflush_wb(&mut self, i: usize) {
            if self.uc {
                return;
            }
            let l = Self::line(i);
            if let Some((a, dirty)) = self.cache.remove(&l) {
                if dirty {
                    for k in 0..PL {
                        self.ram[l * PL + k] = a[k];
                    }
                }
            }
        }
    }

    fn run(uc: bool, total: u32) -> Result<(), String> {
        let mut m = Model {
            ram: vec![D::default(); N],
            cache: BTreeMap::new(),
            uc,
            rdh: 0,
        };
        let mut next = 0usize; // driver next_to_clean
        let mut rdt = N - 1; // last slot handed to NIC
        let mut produced = 0u32;
        let mut received = 0u32;

        // Keep the NIC ~1 descriptor ahead of the driver so it fills same-line
        // neighbours while the driver works — the concurrent-DMA condition.
        let nic_can_fill = |rdh: usize, rdt: usize| (rdh + 1) % N != rdt;

        let mut guard = 0u64;
        while received < total {
            guard += 1;
            if guard > total as u64 * 100 + 1_000_000 {
                return Err(format!(
                    "livelock: received {} of {} (next={}, rdh={})",
                    received, total, next, m.rdh
                ));
            }

            // NIC produces until it is one ahead of the driver (or ring full).
            while produced < total && nic_can_fill(m.rdh, rdt) && (m.rdh + N - next) % N < 2 {
                m.nic_fill(produced);
                produced += 1;
            }

            if next == m.rdh {
                if produced >= total {
                    break;
                }
                continue;
            }

            // Driver processes slot `next`, mirroring the real sequence:
            m.clflush_inval(next); // dma_sync FromDevice
            let d = m.read(next);

            // *** concurrent DMA: NIC fills the next descriptor mid-process ***
            if produced < total && nic_can_fill(m.rdh, rdt) {
                m.nic_fill(produced);
                produced += 1;
            }

            if !d.dd {
                return Err(format!(
                    "RX WEDGED at slot {} after {} of {} packets (DD lost to false sharing)",
                    next, received, total
                ));
            }
            if d.seq != received {
                return Err(format!(
                    "corruption/reorder at slot {}: got seq {}, expected {}",
                    next, d.seq, received
                ));
            }
            received += 1;

            // recycle: clear descriptor and flush its line ToDevice
            m.write(next, D { dd: false, seq: 0 });
            m.clflush_wb(next);
            rdt = next;
            next = (next + 1) % N;
        }

        if received != total {
            return Err(format!("incomplete: {} of {}", received, total));
        }
        Ok(())
    }

    /// >100 MB at 1460 B/packet ~= 72k packets; use 100k. The fix (uncached
    /// descriptor rings) must deliver every packet with no wedge.
    #[test]
    fn uncached_rings_deliver_full_large_download() {
        run(true, 100_000).expect("uncached descriptor rings must not wedge");
    }

    /// Proof the bench actually reproduces the bug: write-back descriptor rings
    /// wedge/corrupt partway through, so this asserts an error is produced.
    #[test]
    fn writeback_rings_reproduce_the_wedge() {
        let r = run(false, 100_000);
        assert!(r.is_err(), "WB rings should reproduce the wedge but passed");
    }

    /// Batch recycle on *write-back* rings: defer recycling until a whole cache
    /// line's PL descriptors are all consumed, then write+flush the line once.
    /// By then the NIC has moved past that line (it fills sequentially and the
    /// line was not returned via RDT yet, so it cannot wrap back), so the
    /// write-back can never clobber a DD the NIC is still setting. This makes
    /// write-back rings safe WITHOUT relying on an uncached remap that may not
    /// take effect on real hardware.
    fn run_batch(total: u32) -> Result<(), String> {
        let mut m = Model {
            ram: vec![D::default(); N],
            cache: BTreeMap::new(),
            uc: false,
            rdh: 0,
        };
        let mut next = 0usize;
        let mut rdt = N - 1;
        let mut produced = 0u32;
        let mut received = 0u32;
        let nic_can_fill = |rdh: usize, rdt: usize| (rdh + 1) % N != rdt;
        let mut guard = 0u64;
        while received < total {
            guard += 1;
            if guard > total as u64 * 100 + 1_000_000 {
                return Err(format!(
                    "livelock: received {} of {} (next={}, rdh={})",
                    received, total, next, m.rdh
                ));
            }
            while produced < total && nic_can_fill(m.rdh, rdt) && (m.rdh + N - next) % N < 2 {
                m.nic_fill(produced);
                produced += 1;
            }
            if next == m.rdh {
                if produced >= total {
                    break;
                }
                continue;
            }
            m.clflush_inval(next);
            let d = m.read(next);
            if produced < total && nic_can_fill(m.rdh, rdt) {
                m.nic_fill(produced);
                produced += 1;
            }
            if !d.dd {
                return Err(format!(
                    "RX WEDGED (batch) at slot {} after {} of {} packets",
                    next, received, total
                ));
            }
            if d.seq != received {
                return Err(format!(
                    "corruption (batch) at slot {}: got seq {}, expected {}",
                    next, d.seq, received
                ));
            }
            received += 1;
            // Recycle the whole cache line only once its last descriptor is done.
            if next % PL == PL - 1 {
                let l0 = next - (PL - 1);
                for s in l0..=next {
                    m.write(s, D { dd: false, seq: 0 });
                }
                m.clflush_wb(next);
                rdt = next;
            }
            next = (next + 1) % N;
        }
        if received != total {
            return Err(format!("incomplete: {} of {}", received, total));
        }
        Ok(())
    }

    /// The write-back-safe fix: cache-line batch recycle delivers the whole
    /// >100 MB stream with no wedge, even though the rings are write-back and
    /// the NIC DMAs concurrently. This does not depend on an uncached remap.
    #[test]
    fn batch_recycle_survives_large_download_on_writeback_rings() {
        run_batch(100_000).expect("batch recycle must not wedge on write-back rings");
    }
}
