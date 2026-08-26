//! PCH (I217/I218/I219) PHY + link bring-up for [`super::e1000e`].
//!
//! Little Kernel's e1000 driver does not implement this; without ULP exit, SWFLAG/MDIO,
//! and copper autoneg the i219 (`8086:15b8`) stays at `link=down` on real boards.

use super::timer_now_as_micros;
use pci::Location;
use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};

#[inline]
unsafe fn mmio_read(base: usize, reg: usize) -> u32 {
    core::ptr::read_volatile((base + reg * 4) as *const u32)
}

#[inline]
unsafe fn mmio_write(base: usize, reg: usize, val: u32) {
    core::ptr::write_volatile((base + reg * 4) as *mut u32, val);
}

const E1000_CTRL: usize = 0x0000 / 4;
const E1000_STATUS: usize = 0x0008 / 4;

// ---------------------------------------------------------------------------
// Registers (byte / 4)
// ---------------------------------------------------------------------------
const E1000_MDIC: usize = 0x0020 / 4;
const E1000_CTRL_EXT: usize = 0x0018 / 4;
const E1000_EXTCNF_CTRL: usize = 0x0F00 / 4;
const E1000_FEXTNVM3: usize = 0x003C / 4;
const E1000_FEXTNVM7: usize = 0x01018 / 4;
const E1000_RFCTL: usize = 0x5008 / 4;
const E1000_FWSM: usize = 0x05B54 / 4;
const E1000_H2ME: usize = 0x05B50 / 4;
/// MAC-side PHY power control: LPLU / GbE-disable bits (Linux E1000_PHY_CTRL).
const E1000_PHY_CTRL: usize = 0x00F10 / 4;

const CTRL_SLU: u32 = 1 << 6;
const CTRL_ASDE: u32 = 1 << 5;
const CTRL_FD: u32 = 1 << 0;
const CTRL_PHY_RST: u32 = 1 << 31;
const CTRL_LANPHYPC_OVERRIDE: u32 = 0x0001_0000;
const CTRL_LANPHYPC_VALUE: u32 = 0x0002_0000;

const STATUS_LU: u32 = 1 << 1;
const STATUS_PHYRA: u32 = 1 << 10;
const STATUS_LAN_INIT_DONE: u32 = 1 << 9;
/// STATUS bits [7:6]: 00=10M 01=100M 10=1G
const STATUS_SPEED_MASK: u32 = 3 << 6;
const STATUS_SPEED_1000: u32 = 2 << 6;

const CTRL_EXT_DRV_LOAD: u32 = 0x1000_0000;
const CTRL_EXT_FORCE_SMBUS: u32 = 0x0000_0800;
const CTRL_EXT_PHYPDEN: u32 = 1 << 20;
const CTRL_EXT_RO_DIS: u32 = 1 << 17; // Relaxation Order Disable (Linux: E1000_CTRL_EXT_RO_DIS = 0x00020000)
const CTRL_EXT_LPCD: u32 = 1 << 2; // Link Partner Connection Detection

const EXTCNF_CTRL_SWFLAG: u32 = 0x20;
const EXTCNF_CTRL_GATE_PHY_CFG: u32 = 0x80;

const FWSM_FW_VALID: u32 = 0x8000;
const FWSM_ULP_CFG_DONE: u32 = 1 << 10;
const FWSM_RSPCIPHY: u32 = 0x40;

const H2ME_ULP: u32 = 0x0000_0800;
const H2ME_ENFORCE_SETTINGS: u32 = 0x0000_1000;

const FEXTNVM3_PHY_CFG_COUNTER_MASK: u32 = 0x0C00_0000;
const FEXTNVM3_PHY_CFG_COUNTER_50MSEC: u32 = 0x0800_0000;
const FEXTNVM7_SIDE_CLK_UNGATE: u32 = 1 << 2;
const FEXTNVM7_DISABLE_SMB_PERST: u32 = 1 << 5;

const RFCTL_NFSW_DIS: u32 = 1 << 6;
const RFCTL_NFSR_DIS: u32 = 1 << 7;

/// MAC-side LPLU/GbE-disable bits in E1000_PHY_CTRL (Linux ich8lan.c).
const PHY_CTRL_D0A_LPLU: u32 = 1 << 1;
const PHY_CTRL_NOND0A_LPLU: u32 = 1 << 2;
const PHY_CTRL_NOND0A_GBE_DISABLE: u32 = 1 << 3;
const PHY_CTRL_GBE_DISABLE: u32 = 1 << 6;

const MDIC_REG_SHIFT: u32 = 16;
const MDIC_PHY_SHIFT: u32 = 21;
const MDIC_OP_READ: u32 = 0x0800_0000;
const MDIC_OP_WRITE: u32 = 0x0400_0000;
const MDIC_READY: u32 = 0x1000_0000;
const MDIC_ERROR: u32 = 0x4000_0000;

const MII_BMCR: u32 = 0;
const MII_BMSR: u32 = 1;
const MII_ADVERTISE: u32 = 4;
const MII_CTRL1000: u32 = 9;
const IGP_PHY_PAGE_SELECT: u32 = 31;

const BMCR_ANENABLE: u16 = 0x1000;
const BMCR_ANRESTART: u16 = 0x0200;
const BMSR_ANEG_COMPLETE: u16 = 0x0020;

const ADVERTISE_CSMA: u16 = 0x0001;
const ADVERTISE_10HALF: u16 = 0x0020;
const ADVERTISE_10FULL: u16 = 0x0040;
const ADVERTISE_100HALF: u16 = 0x0080;
const ADVERTISE_100FULL: u16 = 0x0100;
const ADVERTISE_PAUSE_CAP: u16 = 0x0400;
const ADVERTISE_PAUSE_ASYM: u16 = 0x0800;
const ADVERTISE_ALL_COPPER: u16 = ADVERTISE_CSMA
    | ADVERTISE_10HALF
    | ADVERTISE_10FULL
    | ADVERTISE_100HALF
    | ADVERTISE_100FULL
    | ADVERTISE_PAUSE_CAP
    | ADVERTISE_PAUSE_ASYM;
const ADVERTISE_1000FULL: u16 = 0x0200;

const LANPHYPC_POWERDOWN_HOLD_US: u64 = 20_000;
const LANPHYPC_POWERUP_SETTLE_US: u64 = 200_000;

const MDIC_POLL_TRIES: u32 = 400;
const MDIC_INIT_TRIES: u32 = 48;
const PHY_AUTO_NEG_LIMIT: u16 = 50;

pub struct PchBringupResult {
    pub link_up: bool,
    pub phy_addr: u8,
}

pub fn is_pch_device(device_id: u16) -> bool {
    matches!(
        device_id,
        0x1502..=0x1503
            | 0x153a..=0x153b
            | 0x155a
            | 0x1559
            | 0x15a0..=0x15a3
            | 0x156f..=0x1570
            | 0x15b7..=0x15be
            | 0x15d6..=0x15d8
            | 0x15df..=0x15e2
            | 0x15e3
            | 0x15f4..=0x15fc
            | 0x1a1c..=0x1a1f
            | 0x0d4c..=0x0d4f
            | 0x0d53
            | 0x0d55
            | 0x0dc5..=0x0dc8
            | 0x550a..=0x5511
            | 0x57a0..=0x57a1
            | 0x57b3..=0x57ba
    )
}

struct PchCtx {
    base: usize,
    pci_loc: Location,
    device_id: u16,
    phy_addr: u8,
}

impl PchCtx {
    fn udelay(us: u64) {
        let t0 = timer_now_as_micros();
        let mut spins = 0u64;
        while timer_now_as_micros().wrapping_sub(t0) < us {
            core::hint::spin_loop();
            spins = spins.wrapping_add(1);
            if spins >= 10_000_000 {
                break;
            }
        }
    }

    #[inline]
    fn phy_reg_paged(page: u32, reg: u32) -> u32 {
        (page << 5) | reg
    }

    unsafe fn mdic_clear_stuck(&self) {
        let mdic = mmio_read(self.base, E1000_MDIC);
        if mdic & MDIC_READY != 0 || mdic == 0 {
            return;
        }
        mmio_write(self.base, E1000_MDIC, 0);
        let _ = mmio_read(self.base, E1000_MDIC);
        Self::udelay(100);
    }

    unsafe fn mdic_read_swheld(&self, phy: u8, reg: u32, tries: u32) -> Option<u16> {
        mmio_write(
            self.base,
            E1000_MDIC,
            (reg << MDIC_REG_SHIFT) | ((phy as u32) << MDIC_PHY_SHIFT) | MDIC_OP_READ,
        );
        for _ in 0..tries {
            Self::udelay(50);
            let mdic = mmio_read(self.base, E1000_MDIC);
            if mdic & MDIC_READY != 0 {
                if mdic & MDIC_ERROR == 0 {
                    return Some((mdic & 0xFFFF) as u16);
                }
                self.mdic_clear_stuck();
                return None;
            }
        }
        self.mdic_clear_stuck();
        None
    }

    unsafe fn mdic_write_swheld(&self, phy: u8, reg: u32, val: u16, tries: u32) -> bool {
        mmio_write(
            self.base,
            E1000_MDIC,
            (val as u32)
                | (reg << MDIC_REG_SHIFT)
                | ((phy as u32) << MDIC_PHY_SHIFT)
                | MDIC_OP_WRITE,
        );
        for _ in 0..tries {
            Self::udelay(50);
            let mdic = mmio_read(self.base, E1000_MDIC);
            if mdic & MDIC_READY != 0 {
                if mdic & MDIC_ERROR == 0 {
                    return true;
                }
                self.mdic_clear_stuck();
                return false;
            }
        }
        self.mdic_clear_stuck();
        false
    }

    unsafe fn mdic_read_phy_swheld(&self, phy: u8, offset: u32) -> Option<u16> {
        if offset > 0x1F {
            if !self.mdic_write_swheld(phy, IGP_PHY_PAGE_SELECT, offset as u16, MDIC_POLL_TRIES) {
                return None;
            }
        }
        self.mdic_read_swheld(phy, offset & 0x1F, MDIC_POLL_TRIES)
    }

    unsafe fn mdic_write_phy_swheld(&self, phy: u8, offset: u32, val: u16) -> bool {
        if offset > 0x1F {
            if !self.mdic_write_swheld(phy, IGP_PHY_PAGE_SELECT, offset as u16, MDIC_POLL_TRIES) {
                return false;
            }
        }
        self.mdic_write_swheld(phy, offset & 0x1F, val, MDIC_POLL_TRIES)
    }

    unsafe fn swflag_acquire_init(&self) -> bool {
        for _ in 0..1000 {
            if mmio_read(self.base, E1000_EXTCNF_CTRL) & EXTCNF_CTRL_SWFLAG == 0 {
                break;
            }
            Self::udelay(1_000);
        }
        let mut ext = mmio_read(self.base, E1000_EXTCNF_CTRL);
        if ext & EXTCNF_CTRL_SWFLAG != 0 {
            ext &= !EXTCNF_CTRL_SWFLAG;
            mmio_write(self.base, E1000_EXTCNF_CTRL, ext);
            let _ = mmio_read(self.base, E1000_EXTCNF_CTRL);
            Self::udelay(200_000);
        }
        ext = mmio_read(self.base, E1000_EXTCNF_CTRL);
        ext |= EXTCNF_CTRL_SWFLAG;
        mmio_write(self.base, E1000_EXTCNF_CTRL, ext);
        for _ in 0..1000 {
            if mmio_read(self.base, E1000_EXTCNF_CTRL) & EXTCNF_CTRL_SWFLAG != 0 {
                return true;
            }
            Self::udelay(1_000);
        }
        ext &= !EXTCNF_CTRL_SWFLAG;
        mmio_write(self.base, E1000_EXTCNF_CTRL, ext);
        false
    }

    unsafe fn swflag_release(&self) {
        let mut v = mmio_read(self.base, E1000_EXTCNF_CTRL);
        if v & EXTCNF_CTRL_SWFLAG != 0 {
            v &= !EXTCNF_CTRL_SWFLAG;
            mmio_write(self.base, E1000_EXTCNF_CTRL, v);
        }
    }

    unsafe fn mdio_prepare(&self) {
        let mut ext = mmio_read(self.base, E1000_EXTCNF_CTRL);
        if ext & EXTCNF_CTRL_GATE_PHY_CFG != 0 {
            ext &= !EXTCNF_CTRL_GATE_PHY_CFG;
            mmio_write(self.base, E1000_EXTCNF_CTRL, ext);
            Self::udelay(1_000);
        }
        self.mdic_clear_stuck();
    }

    unsafe fn set_drv_load(&self) {
        let mut ctrl_ext = mmio_read(self.base, E1000_CTRL_EXT);
        if ctrl_ext & CTRL_EXT_DRV_LOAD == 0 {
            ctrl_ext |= CTRL_EXT_DRV_LOAD;
            mmio_write(self.base, E1000_CTRL_EXT, ctrl_ext);
            let _ = mmio_read(self.base, E1000_CTRL_EXT);
            Self::udelay(100_000);
        }
    }

    unsafe fn clear_status_phyra(&self) {
        let s = mmio_read(self.base, E1000_STATUS);
        if s & STATUS_PHYRA != 0 {
            mmio_write(self.base, E1000_STATUS, s & !STATUS_PHYRA);
        }
    }

    /// Linux `set_d0_lplu_state_ich8lan(false)` — clear MAC-side LPLU and GbE-disable.
    /// Must be called after any PHY reset so the PHY advertises all speeds including 1G.
    /// Without this, I219 may only advertise 10M (LPLU default after power-up/reset).
    unsafe fn clear_lplu_mac(&self) {
        let mut phy_ctrl = mmio_read(self.base, E1000_PHY_CTRL);
        let before = phy_ctrl;
        phy_ctrl &= !(PHY_CTRL_D0A_LPLU
            | PHY_CTRL_NOND0A_LPLU
            | PHY_CTRL_GBE_DISABLE
            | PHY_CTRL_NOND0A_GBE_DISABLE);
        if phy_ctrl != before {
            mmio_write(self.base, E1000_PHY_CTRL, phy_ctrl);
            let _ = mmio_read(self.base, E1000_PHY_CTRL);
            crate::klog_warn!(
                "[e1000e] PCH: cleared LPLU/GBE-dis PHY_CTRL {:#x}->{:#x}\n",
                before,
                phy_ctrl
            );
        }
    }

    unsafe fn disable_ulp_me(&self) -> bool {
        let fwsm = mmio_read(self.base, E1000_FWSM);
        if fwsm & FWSM_FW_VALID == 0 {
            return false;
        }
        let mut h2me = mmio_read(self.base, E1000_H2ME);
        h2me &= !H2ME_ULP;
        h2me |= H2ME_ENFORCE_SETTINGS;
        mmio_write(self.base, E1000_H2ME, h2me);
        for i in 0..250u32 {
            if mmio_read(self.base, E1000_FWSM) & FWSM_ULP_CFG_DONE == 0 {
                if i > 0 {
                    crate::klog_warn!("[e1000e] ME cleared ULP in {} ms\n", i * 10);
                }
                h2me = mmio_read(self.base, E1000_H2ME);
                h2me &= !H2ME_ENFORCE_SETTINGS;
                mmio_write(self.base, E1000_H2ME, h2me);
                return true;
            }
            Self::udelay(10_000);
        }
        false
    }

    unsafe fn disable_ulp_sw(&self, phy: u8) -> bool {
        let cv = Self::phy_reg_paged(769, 23);
        let mut ok = false;
        if let Some(mut r) = self.mdic_read_phy_swheld(phy, cv) {
            r &= !0x0001;
            ok = self.mdic_write_phy_swheld(phy, cv, r);
        }
        let mut ctrl_ext = mmio_read(self.base, E1000_CTRL_EXT);
        ctrl_ext &= !CTRL_EXT_PHYPDEN;
        mmio_write(self.base, E1000_CTRL_EXT, ctrl_ext);

        let ulp = Self::phy_reg_paged(779, 16);
        if let Some(mut r) = self.mdic_read_phy_swheld(phy, ulp) {
            r &= !0x1D74;
            let _ = self.mdic_write_phy_swheld(phy, ulp, r);
            r |= 0x0001;
            let _ = self.mdic_write_phy_swheld(phy, ulp, r);
        }

        if let Some(bmsr) = self.mdic_read_swheld(phy, MII_BMSR, MDIC_INIT_TRIES) {
            crate::klog_warn!("[e1000e] ULP off PHY{} BMSR={:#x}\n", phy, bmsr);
            ok = true;
        }
        ok
    }

    unsafe fn toggle_lanphypc(&self) {
        let mut f3 = mmio_read(self.base, E1000_FEXTNVM3);
        f3 &= !FEXTNVM3_PHY_CFG_COUNTER_MASK;
        f3 |= FEXTNVM3_PHY_CFG_COUNTER_50MSEC;
        mmio_write(self.base, E1000_FEXTNVM3, f3);

        let mut ctrl = mmio_read(self.base, E1000_CTRL);
        ctrl |= CTRL_LANPHYPC_OVERRIDE;
        ctrl &= !CTRL_LANPHYPC_VALUE;
        mmio_write(self.base, E1000_CTRL, ctrl);
        Self::udelay(LANPHYPC_POWERDOWN_HOLD_US);

        ctrl = mmio_read(self.base, E1000_CTRL);
        ctrl &= !CTRL_LANPHYPC_OVERRIDE;
        mmio_write(self.base, E1000_CTRL, ctrl);

        for _ in 0..40 {
            if mmio_read(self.base, E1000_CTRL_EXT) & CTRL_EXT_LPCD != 0 {
                break;
            }
            Self::udelay(5_000);
        }
        Self::udelay(LANPHYPC_POWERUP_SETTLE_US);
    }

    unsafe fn phy_reset_blocked(&self) -> bool {
        for _ in 0..30 {
            if mmio_read(self.base, E1000_FWSM) & FWSM_RSPCIPHY != 0 {
                return false;
            }
            Self::udelay(10_000);
        }
        true
    }

    unsafe fn issue_phy_reset(&self) {
        if !self.swflag_acquire_init() {
            return;
        }
        let ctrl = mmio_read(self.base, E1000_CTRL);
        mmio_write(self.base, E1000_CTRL, ctrl | CTRL_PHY_RST);
        Self::udelay(100);
        mmio_write(self.base, E1000_CTRL, ctrl);
        Self::udelay(300_000);
        self.swflag_release();
        self.clear_status_phyra();
    }

    unsafe fn setup_copper_mac(&self) {
        let mut ctrl = mmio_read(self.base, E1000_CTRL);
        ctrl |= CTRL_SLU | CTRL_ASDE | CTRL_FD;
        ctrl &= !((1 << 11) | (1 << 12)); // FRCSPD | FRCDPX
        mmio_write(self.base, E1000_CTRL, ctrl);
    }

    unsafe fn apply_spt_workarounds(&self) {
        let mut rfctl = mmio_read(self.base, E1000_RFCTL);
        rfctl |= RFCTL_NFSW_DIS | RFCTL_NFSR_DIS;
        mmio_write(self.base, E1000_RFCTL, rfctl);

        let mut f7 = mmio_read(self.base, E1000_FEXTNVM7);
        f7 |= FEXTNVM7_SIDE_CLK_UNGATE | FEXTNVM7_DISABLE_SMB_PERST;
        mmio_write(self.base, E1000_FEXTNVM7, f7);

        let mut ext = mmio_read(self.base, E1000_CTRL_EXT);
        ext |= CTRL_EXT_RO_DIS;
        ext &= !CTRL_EXT_PHYPDEN;
        mmio_write(self.base, E1000_CTRL_EXT, ext);
    }

    unsafe fn mdic_with_swflag<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&Self) -> Option<R>,
    {
        if !self.swflag_acquire_init() {
            return None;
        }
        let r = f(self);
        self.swflag_release();
        r
    }

    unsafe fn restart_autoneg(&self, phy: u8) -> bool {
        self.mdic_with_swflag(|ctx| {
            if !ctx.mdic_write_swheld(phy, MII_ADVERTISE, ADVERTISE_ALL_COPPER, MDIC_POLL_TRIES) {
                return None;
            }
            if !ctx.mdic_write_swheld(phy, MII_CTRL1000, ADVERTISE_1000FULL, MDIC_POLL_TRIES) {
                return None;
            }
            let bmcr = ctx.mdic_read_swheld(phy, MII_BMCR, MDIC_POLL_TRIES)?;
            if bmcr == 0 || bmcr == 0xFFFF {
                return None;
            }
            if !ctx.mdic_write_swheld(
                phy,
                MII_BMCR,
                bmcr | BMCR_ANENABLE | BMCR_ANRESTART,
                MDIC_POLL_TRIES,
            ) {
                return None;
            }
            Some(())
        })
        .is_some()
    }

    unsafe fn wait_autoneg(&self, phy: u8) {
        let mut i = PHY_AUTO_NEG_LIMIT;
        while i > 0 {
            let done = if let Some(bmsr) = self.mdic_with_swflag(|ctx| {
                ctx.mdic_read_swheld(phy, MII_BMSR, MDIC_INIT_TRIES)
            }) {
                bmsr & BMSR_ANEG_COMPLETE != 0
            } else {
                false
            };
            if done {
                return;
            }
            Self::udelay(100_000);
            i -= 1;
        }
    }

    /// Wait for STATUS.LU, preferring 1G: continues polling even if 10/100M link appears,
    /// giving the autoneg master/slave resolution time to reach 1G before we commit.
    /// Returns true if any link came up within budget_us.
    unsafe fn wait_status_link_prefer_gig(&self, budget_us: u64) -> bool {
        let t0 = timer_now_as_micros();
        let mut got_link = false;
        while timer_now_as_micros().wrapping_sub(t0) < budget_us {
            let status = mmio_read(self.base, E1000_STATUS);
            if status & STATUS_LU != 0 {
                got_link = true;
                if (status & STATUS_SPEED_MASK) == STATUS_SPEED_1000 {
                    // 1G achieved — stop waiting
                    return true;
                }
                // Link at 10/100M: keep polling; 1G master/slave may still be resolving
            }
            Self::udelay(50_000);
        }
        got_link
    }

    unsafe fn restore_pci_bus_master(&self) {
        let mut cmd = PCI_ACCESS.read16(&PortOpsImpl, self.pci_loc, 0x04);
        cmd |= 0x0006;
        PCI_ACCESS.write16(&PortOpsImpl, self.pci_loc, 0x04, cmd);
    }

    unsafe fn bringup(&mut self) -> PchBringupResult {
        self.set_drv_load();
        self.clear_status_phyra();
        self.mdio_prepare();
        self.restore_pci_bus_master();

        if !self.disable_ulp_me() {
            if self.swflag_acquire_init() {
                self.toggle_lanphypc();
                let _ = self.disable_ulp_sw(self.phy_addr);
                if !self.phy_reset_blocked() {
                    self.issue_phy_reset();
                }
                self.swflag_release();
            }
        }

        self.setup_copper_mac();
        self.apply_spt_workarounds();

        // Clear MAC-side LPLU/GbE-disable AFTER any PHY reset so the PHY
        // re-advertises all speeds. Without this, I219 defaults to LPLU (10M only).
        self.clear_lplu_mac();

        let mut linked = false;
        for phy in [2u8, 1u8] {
            if self.restart_autoneg(phy) {
                self.phy_addr = phy;
                self.wait_autoneg(phy);
                // Use 1G-preferring wait: keeps polling even if 10M link appears first
                linked = self.wait_status_link_prefer_gig(3_000_000);
                if linked {
                    break;
                }
            }
        }

        if !linked {
            crate::klog_warn!(
                "[e1000e] PCH link still down STATUS={:#x} FWSM={:#x} EXTCNF={:#x}\n",
                mmio_read(self.base, E1000_STATUS),
                mmio_read(self.base, E1000_FWSM),
                mmio_read(self.base, E1000_EXTCNF_CTRL)
            );
        } else {
            crate::klog_warn!(
                "[e1000e] PCH link up STATUS={:#x} PHY={}\n",
                mmio_read(self.base, E1000_STATUS),
                self.phy_addr
            );
        }

        PchBringupResult {
            link_up: linked,
            phy_addr: self.phy_addr,
        }
    }
}

/// Linux `e1000_init_phy_workarounds_pchlan` + autoneg subset for I219-class PCH.
pub unsafe fn bringup_link(base: usize, pci_loc: Location, device_id: u16) -> PchBringupResult {
    if !is_pch_device(device_id) {
        return PchBringupResult {
            link_up: mmio_read(base, E1000_STATUS) & STATUS_LU != 0,
            phy_addr: 1,
        };
    }
    let mut ctx = PchCtx {
        base,
        pci_loc,
        device_id,
        phy_addr: if device_id >= 0x15b7 && device_id <= 0x15be {
            2
        } else {
            1
        },
    };
    ctx.bringup()
}
