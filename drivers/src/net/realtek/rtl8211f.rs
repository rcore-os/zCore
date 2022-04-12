// Supports Realtek RTL8211F on Allwinner D1

extern crate alloc;

use super::mii::*;
use super::utils::*;

use core::marker::PhantomData;
use core::mem::size_of;

use super::Provider;
use super::{phys_to_virt, virt_to_phys};
use alloc::boxed::Box;
use alloc::slice;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

// RTL8211F
pub const GMAC_BASE: u32 = 0x04500000;
pub const CCU_BASE: u32 = 0x02001000;
pub const SYS_CFG_BASE: u32 = 0x03000000;
pub const PINCTRL_GPIO_BASE: u32 = 0x02000000;

const EMAC_BGR_REG: u32 = 0x097C; // CCU
const EMAC_25M_CLK_REG: u32 = 0x0970;

const EMAC_EPHY_CLK_REG0: u32 = 0x30; // SYS_CFG

// mac addr 3a:c5:31:d5:de:88
const MAC_ADDR: &str = "3a:c5:31:d5:de:88";

const DMA_DESC_RX: usize = 256;
const DMA_DESC_TX: usize = 256;
const BUDGET: usize = DMA_DESC_RX / 4;
const TX_THRESH: usize = DMA_DESC_TX / 4;

const MAX_BUF_SZ: u32 = 2048 - 1;

const TX_DELAY: u32 = 3;
const RX_DELAY: u32 = 0;

const MDC_CLOCK_RATIO: u32 = 0x03;

const GETH_BASIC_CTL0: u32 = 0x00;
const GETH_BASIC_CTL1: u32 = 0x04;
const GETH_INT_STA: u32 = 0x08;
const GETH_INT_EN: u32 = 0x0C;
const GETH_TX_CTL0: u32 = 0x10;
const GETH_TX_CTL1: u32 = 0x14;
const GETH_TX_FLOW_CTL: u32 = 0x1C;
const GETH_TX_DESC_LIST: u32 = 0x20;
const GETH_RX_CTL0: u32 = 0x24;
const GETH_RX_CTL1: u32 = 0x28;
const GETH_RX_DESC_LIST: u32 = 0x34;
const GETH_RX_FRM_FLT: u32 = 0x38;
const GETH_RX_HASH0: u32 = 0x40;
const GETH_RX_HASH1: u32 = 0x44;
const GETH_MDIO_ADDR: u32 = 0x48;
const GETH_MDIO_DATA: u32 = 0x4C;
const GETH_ADDR_HI: u32 = 0x50; //(0x50 + ((reg) << 3))
const GETH_ADDR_LO: u32 = 0x54; //(0x54 + ((reg) << 3))
const GETH_TX_DMA_STA: u32 = 0xB0;
const GETH_TX_CUR_DESC: u32 = 0xB4;
const GETH_TX_CUR_BUF: u32 = 0xB8;
const GETH_RX_DMA_STA: u32 = 0xC0;
const GETH_RX_CUR_DESC: u32 = 0xC4;
const GETH_RX_CUR_BUF: u32 = 0xC8;
const GETH_RGMII_STA: u32 = 0xD0;

const RGMII_IRQ: u32 = 0x00000001;

const MII: usize = 2;
const GMII: usize = 3;
const RMII: usize = 7;
const RGMII: usize = 8;

const CTL0_LM: u32 = 0x02;
const CTL0_DM: u32 = 0x01;
const CTL0_SPEED: u32 = 0x04;

const BURST_LEN: u32 = 0x3F000000;
const RX_TX_PRI: u32 = 0x02;
const SOFT_RST: u32 = 0x01;

const TX_FLUSH: u32 = 0x01;
const TX_MD: u32 = 0x02;
const TX_NEXT_FRM: u32 = 0x04;
const TX_TH: u32 = 0x0700;

const RX_FLUSH: u32 = 0x01;
const RX_MD: u32 = 0x02;
const RX_RUNT_FRM: u32 = 0x04;
const RX_ERR_FRM: u32 = 0x08;
const RX_TH: u32 = 0x0030;

const TX_INT: u32 = 0x00001;
const TX_STOP_INT: u32 = 0x00002;
const TX_UA_INT: u32 = 0x00004;
const TX_TOUT_INT: u32 = 0x00008;
const TX_UNF_INT: u32 = 0x00010;
const TX_EARLY_INT: u32 = 0x00020;
const RX_INT: u32 = 0x00100;
const RX_UA_INT: u32 = 0x00200;
const RX_STOP_INT: u32 = 0x00400;
const RX_TOUT_INT: u32 = 0x00800;
const RX_OVF_INT: u32 = 0x01000;
const RX_EARLY_INT: u32 = 0x02000;
const LINK_STA_INT: u32 = 0x10000;

const SPEED_10: i32 = 10;
const SPEED_100: i32 = 100;
const SPEED_1000: i32 = 1000;
const SPEED_UNKNOWN: i32 = -1;

const DUPLEX_HALF: i32 = 0x00;
const DUPLEX_FULL: i32 = 0x01;
const DUPLEX_UNKNOWN: i32 = 0xff;

const SF_DMA_MODE: usize = 1;

/* - 0: Flow Off
 * - 1: Rx Flow
 * - 2: Tx Flow
 * - 3: Rx & Tx Flow
 */
const FLOW_CTRL: u32 = 0;
const PAUSE: u32 = 0x400;

/* Enable or disable autonegotiation. */
const AUTONEG_DISABLE: usize = 0;
const AUTONEG_ENABLE: usize = 1;

/* Flow Control defines */
const FLOW_OFF: u32 = 0;
const FLOW_RX: u32 = 1;
const FLOW_TX: u32 = 2;
const FLOW_AUTO: u32 = (FLOW_TX | FLOW_RX);
const HASH_TABLE_SIZE: u32 = 64;
const PAUSE_TIME: u32 = 0x200;
const GMAC_MAX_UNICAST_ADDRESSES: u32 = 8;

/* PHY address */
const PHY_ADDR: u32 = 0x01;
const PHY_DM: u32 = 0x0010;
const PHY_AUTO_NEG: u32 = 0x0020;
const PHY_POWERDOWN: u32 = 0x0080;
const PHY_NEG_EN: u32 = 0x1000;

const MII_BUSY: u32 = 0x00000001;
const MII_WRITE: u32 = 0x00000002;
const MII_PHY_MASK: u32 = 0x0000FFC0;
const MII_CR_MASK: u32 = 0x0000001C;
const MII_CLK: u32 = 0x00000008;

const MII_BMCR: u32 = 0x00;
const BMCR_RESET: u32 = 0x8000;
const BMCR_PDOWN: u32 = 0x0800;

#[derive(Debug, Copy, Clone)]
#[repr(packed)]
pub struct dma_desc {
    // size: 16
    desc0: u32, // Status
    desc1: u32, // Buffer Size
    desc2: u32, // Buffer Addr
    desc3: u32, // Next Desc
}

pub enum rx_frame_status {
    /* IPC status */
    good_frame = 0,
    discard_frame = 1,
    csum_none = 2,
    llc_snap = 4,
}

pub enum tx_dma_irq_status {
    tx_hard_error = 1,
    tx_hard_error_bump_tc = 2,
    handle_tx_rx = 3,
}

pub struct RTL8211F<P: Provider> {
    base: u32,     // 0x4500000
    base_ccu: u32, // CCU_BASE
    base_phy: u32, // SYS_CFG

    pinctrl: u32, // 0x2000000

    mac: [u8; 6],
    recv_buffers: Vec<usize>,
    recv_ring: &'static mut [dma_desc],

    send_buffers: Vec<usize>,
    send_ring: &'static mut [dma_desc],

    phy_mode: usize,

    autoneg: usize,

    tx_delay: u32,
    rx_delay: u32,

    tx_dirty: usize,
    tx_clean: usize,
    rx_dirty: usize,
    rx_clean: usize,

    marker: PhantomData<P>,
}

impl<P> RTL8211F<P>
where
    P: Provider,
{
    #[allow(clippy::clone_on_copy)]
    pub fn new(mac_addr: &[u8; 6]) -> Self {
        assert_eq!(size_of::<dma_desc>(), 16);

        let mut mac: [u8; 6] = [0; 6];
        let v_addr = mac_addr[0] as u32
            + mac_addr[1] as u32
            + mac_addr[2] as u32
            + mac_addr[3] as u32
            + mac_addr[4] as u32
            + mac_addr[5] as u32;
        if (v_addr == 0) || // mac addr is all 0
           ((mac_addr[0] & 0x01) == 1) || // mac addr is multicast
           (v_addr == 0x5fa)
        // mac addr is broadcast
        {
            let tokens: Vec<&str> = MAC_ADDR.split(':').collect();
            for (i, s) in tokens.iter().enumerate() {
                mac[i] = u8::from_str_radix(s, 16).unwrap();
            }
        } else {
            mac = *mac_addr;
        }

        info!(
            "mac addr: {:x}:{:x}:{:x}:{:x}:{:x}:{:x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );

        // DMA使用的dma_desc内存，有一致性要求，一般非cache的
        // 而这里到时会flush_cache()来同步cache
        // dma_desc记得内存清零
        let (send_ring_va, send_ring_pa) = P::alloc_dma(P::PAGE_SIZE);
        let (recv_ring_va, recv_ring_pa) = P::alloc_dma(P::PAGE_SIZE);
        let send_ring = unsafe {
            slice::from_raw_parts_mut(
                send_ring_va as *mut dma_desc,
                P::PAGE_SIZE / size_of::<dma_desc>(), // 4096/16 = 256 个 dma_desc
            )
        };

        let recv_ring = unsafe {
            slice::from_raw_parts_mut(
                recv_ring_va as *mut dma_desc,
                P::PAGE_SIZE / size_of::<dma_desc>(),
            )
        };

        send_ring.fill(dma_desc {
            desc0: 0,
            desc1: 0,
            desc2: 0,
            desc3: 0,
        });
        recv_ring.fill(dma_desc {
            desc0: 0,
            desc1: 0,
            desc2: 0,
            desc3: 0,
        });

        let mut send_buffers = Vec::with_capacity(send_ring.len());
        let mut recv_buffers = Vec::with_capacity(recv_ring.len());

        info!("Set a ring desc buffer for TX");
        // Set a ring desc buffer for TX
        for i in 0..send_ring.len() {
            let (buffer_page_va, buffer_page_pa) = P::alloc_dma(P::PAGE_SIZE); // 其实buffer申请2K左右就可以

            // desc1.all |= (1 << 24) Chain mode
            send_ring[i].desc1 |= (1 << 24);

            send_ring[i].desc2 = buffer_page_pa as u32;

            if (i + 1) == send_ring.len() {
                send_ring[i].desc3 = virt_to_phys(&send_ring[0] as *const dma_desc as usize) as u32;
            } else {
                send_ring[i].desc3 =
                    virt_to_phys(&send_ring[i + 1] as *const dma_desc as usize) as u32;
            }

            send_buffers.push(buffer_page_va);
        }

        info!("Set a ring desc buffer for RX");
        // Set a ring desc buffer for RX
        for i in 0..recv_ring.len() {
            let (buffer_page_va, buffer_page_pa) = P::alloc_dma(P::PAGE_SIZE);

            recv_ring[i].desc1 |= (1 << 24);
            //recv_ring[i].desc2 = buffer_page_pa as u32;
            if (i + 1) == recv_ring.len() {
                recv_ring[i].desc3 = virt_to_phys(&recv_ring[0] as *const dma_desc as usize) as u32;
            } else {
                recv_ring[i].desc3 =
                    virt_to_phys(&recv_ring[i + 1] as *const dma_desc as usize) as u32;
            }

            recv_buffers.push(buffer_page_va);

            // geth_rx_refill, 实际运行refill时却是：priv->rx_clean: 0 ~ 254 ?
            // desc_buf_set(&mut recv_ring[i], buffer_page_pa as u32, MAX_BUF_SZ);
            recv_ring[i].desc1 &= (!((1 << 11) - 1));
            recv_ring[i].desc1 |= (MAX_BUF_SZ & ((1 << 11) - 1));
            recv_ring[i].desc2 = buffer_page_pa as u32;

            // sync memery, fence指令？

            desc_set_own(&mut recv_ring[i]);
        }

        info!(
            "send_buffers length: {}, recv_buffers length: {}",
            send_buffers.len(),
            recv_buffers.len()
        );

        RTL8211F {
            base: GMAC_BASE,
            base_ccu: CCU_BASE,
            base_phy: SYS_CFG_BASE,

            pinctrl: PINCTRL_GPIO_BASE,

            mac,
            recv_buffers,
            recv_ring,

            send_buffers,
            send_ring,

            phy_mode: RGMII,
            autoneg: AUTONEG_ENABLE,
            //autoneg: AUTONEG_DISABLE,
            tx_delay: TX_DELAY,
            rx_delay: RX_DELAY,

            tx_dirty: 0,
            tx_clean: 0,
            rx_dirty: 0,
            rx_clean: 0,

            marker: PhantomData,
        }
    }

    pub fn open(&mut self) -> Result<i32, &str> {
        // 初始化驱动之前设置 pinctrl
        self.pinctrl_gpio_set_gmac();

        // gmacirq 62 --> geth_interrupt()

        // ephy_clk CLK_EMAC0_25M

        if (self.phy_mode != MII) && (self.phy_mode != RGMII) && (self.phy_mode != RMII) {
            error!("Not support phy type !");
            self.phy_mode = MII;
        }

        self.power_on();
        self.clk_enable();

        // geth_phy_init

        /* If config gpio to reset the phy device, we should reset it */
        self.pinctrl_gpio_reset_gmac_phy();

        self.mdio_reset();

        // PHY_POLL = -1, linux驱动，如果不支持中断

        // #define PHY_MAX_ADDR 32
        let phyaddr = 0;
        self.mdio_write(phyaddr, MII_BMCR, BMCR_RESET);
        while (BMCR_RESET & self.mdio_read(phyaddr, MII_BMCR)) != 0 {
            //sleep(30);  // sleep 30 milliseconds
        }

        let mii_bmcr_value = self.mdio_read(phyaddr, MII_BMCR);
        self.mdio_write(phyaddr, MII_BMCR, (mii_bmcr_value & !BMCR_PDOWN));
        info!("Read MII_BMCR: {:#x}", mii_bmcr_value);

        self.mac_reset();

        self.mac_init(1, 1);

        self.set_umac(&self.mac, 0);

        // dma_desc_init
        // Set a ring desc buffer
        // implemented in new()
        //
        self.rx_refill();

        flush_cache(
            virt_to_phys(&self.recv_ring[0] as *const dma_desc as usize) as u64,
            (size_of::<dma_desc>() * self.recv_ring.len()) as u64,
        );
        flush_cache(
            virt_to_phys(&self.send_ring[0] as *const dma_desc as usize) as u64,
            (size_of::<dma_desc>() * self.send_ring.len()) as u64,
        );

        // phy_start
        // 注意地址32位对齐
        self.start_rx(virt_to_phys(&self.recv_ring[0] as *const dma_desc as usize) as u32);
        self.start_tx(virt_to_phys(&self.send_ring[0] as *const dma_desc as usize) as u32);

        // Enable the Rx/Tx
        self.mac_enable();

        Ok(0)
    }

    pub fn pinctrl_gpio_set_gmac(&mut self) {
        // pctl->membase VA: 0xffffffd00405c000, name pinctrl@2000000 PA: 0x2000000

        // PE_CFG0
        self.pinctrl_gpio_set(0xc0, 0x88888888);

        // PE_PULL0, Pull_up/down disable, u-boot
        self.pinctrl_gpio_set(0xe4, 0x0);

        // PE_DRV0, multi driving select level0
        self.pinctrl_gpio_set(0xd4, 0x0);

        // PE_CFG1
        self.pinctrl_gpio_set(0xc4, 0x88888888);

        // PE_PULL0, Pull_up/down disable, u-boot
        self.pinctrl_gpio_set(0xe4, 0x0);

        // PE_DRV1, multi driving select level0
        self.pinctrl_gpio_set(0xd8, 0x0);
    }

    pub fn pinctrl_gpio_reset_gmac_phy(&mut self) {
        // set GPIO direction to output

        // PE Data Register
        // index 16
        self.pinctrl_gpio_set(0xd0, 0x0);

        // PE_CFG2, PE16 select Output
        self.pinctrl_gpio_set(0xc8, 0xf1);

        // sleep 50 milliseconds

        // PE Data Register
        // index 16
        self.pinctrl_gpio_set(0xd0, 0x10000); //第16位？

        // PE_CFG2, PE16 select Output
        self.pinctrl_gpio_set(0xc8, 0xf1);

        // sleep 50 milliseconds
    }

    pub fn set_rx_mode(&mut self) {
        self.set_filter(0x0);

        // Hash Multicast
        self.hash_filter(0x0, 0x1);
        self.set_filter(0x4);

        //self.adjust_link(); // 顺序不必需放这步

        // Pass all multicast
        self.hash_filter(0xffffffff, 0xffffffff);
        self.set_filter(0x10);

        // Promiscuous Mode
        self.set_filter(0x1);
    }

    pub fn adjust_link(&mut self) -> Result<i32, &str> {
        let phyaddr = 0;
        let mut link: u32 = 0;
        let mut autoneg_complete: u32 = 0;

        let mut duplex: i32 = DUPLEX_UNKNOWN;
        let mut speed: i32 = SPEED_UNKNOWN;
        let mut pause: i32 = 0;
        let mut asym_pause: i32 = 0;

        if self.autoneg == AUTONEG_ENABLE {
            // Auto negotiation

            // 在哪设置这__phy_modify_changed ?
            /*
            let setup = self.mdio_read(phyaddr, MII_BMCR);
            setup |= BMCR_SPEED1000;
            self.mdio_write(phyaddr, MII_BMCR, setup);
            */

            /* Setup standard advertisement */
            let adv = ADVERTISE_ALL;
            self.phy_modify(
                MII_ADVERTISE,
                ADVERTISE_ALL | ADVERTISE_100BASE4 | ADVERTISE_PAUSE_CAP | ADVERTISE_PAUSE_ASYM,
                adv,
            );

            // 1000M PHY BMSR_ESTATEN = 1
            let bmcr = self.mdio_read(phyaddr, MII_BMSR);
            if (bmcr & BMSR_ESTATEN) != 0 {
                let adv = ADVERTISE_1000FULL;
                self.phy_modify(MII_CTRL1000, ADVERTISE_1000FULL | ADVERTISE_1000HALF, adv);
            }

            self.phy_restart_aneg();

            // 在AUTONEG，autoneg_complete完成时，就开始解析设置自协商的单双工或速率等信息
            // read LPA todo
            // phy_resolve_aneg_linkmode() TODO 接着解析自协商的匹配速率

            let is_gigabit_capable = 1;
            // 有Gigabit连接能力时
            if is_gigabit_capable != 0 {
                let lpagb = self.mdio_read(phyaddr, MII_STAT1000);
                info!("MII_STAT1000    : {:#x}", lpagb);

                let advgb = self.mdio_read(phyaddr, MII_CTRL1000);
                if (lpagb & LPA_1000MSFAIL) != 0 {
                    if (advgb & CTL1000_ENABLE_MASTER) != 0 {
                        error!(
                            "Master/Slave resolution failed, maybe conflicting manual settings ?"
                        );
                    } else {
                        error!("Master/Slave resolution failed");
                    }
                    return Err("Master/Slave resolution failed ! NOLINK");
                }

                // 这里更新1000M的信息, 等下接着更新100M/10M的信息
                //
                // MII_STAT1000寄存器获取对端的能力： LPA_1000FULL, LPA_1000HALF
                // 没有包括 Pause
                if ((lpagb & LPA_1000FULL) != 0) && ((advgb & ADVERTISE_1000FULL) != 0) {
                    speed = SPEED_1000;
                    duplex = DUPLEX_FULL;
                } else if ((lpagb & LPA_1000HALF) != 0) && ((advgb & ADVERTISE_1000HALF) != 0) {
                    speed = SPEED_1000;
                    duplex = DUPLEX_HALF;
                }
            }
            /////////
            //百兆以下

            let adv = self.mdio_read(phyaddr, MII_ADVERTISE);
            info!("MII_ADVERTISE   : {:#x}", adv);

            // MII_LPA寄存器获取对端的能力： 100/10M, Full/Half, Pause, Asym_Pause
            let mut lpa = self.mdio_read(phyaddr, MII_LPA);
            info!("MII_LPA         : {:#x}", lpa);

            lpa &= adv; // LINK能力，取你我交集
            info!("LPA & ADVERTISE : {:#x}", lpa);

            //speed按从高到低的优先顺序匹配
            if speed == SPEED_1000 {
            } else if (lpa & LPA_100FULL) != 0 {
                speed = SPEED_100;
                duplex = DUPLEX_FULL;
            } else if (lpa & LPA_100HALF) != 0 {
                speed = SPEED_100;
                duplex = DUPLEX_HALF;
            } else if (lpa & LPA_10FULL) != 0 {
                speed = SPEED_10;
                duplex = DUPLEX_FULL;
            } else {
                speed = SPEED_10;
                duplex = DUPLEX_HALF;
            }

            if duplex == DUPLEX_FULL {
                pause = if (lpa & LPA_PAUSE_CAP) != 0 { 1 } else { 0 };
                asym_pause = if (lpa & LPA_PAUSE_ASYM) != 0 { 1 } else { 0 };
            }

            // 解析到speed duplex并设置后,判断网线Link
        } else {
            // AUTONEG_DISABLE

            // Configures MII_BMCR to force speed/duplex
            //let ctl = BMCR_SPEED1000 | BMCR_FULLDPLX;
            // 默认设成 100/FULL
            let ctl = BMCR_SPEED100 | BMCR_FULLDPLX;
            self.phy_modify(MII_BMCR, !(BMCR_LOOPBACK | BMCR_ISOLATE | BMCR_PDOWN), ctl);
            // 开始check网卡link状态, 然后设置speed/duplex

            // genphy_read_status()
            let bmcr: u32 = self.mdio_read(phyaddr, MII_BMCR);
            info!("MII_BMCR: {:#x}", bmcr);
            if (bmcr & BMCR_FULLDPLX) != 0 {
                duplex = DUPLEX_FULL;
            } else {
                duplex = DUPLEX_HALF;
            }
            if (bmcr & BMCR_SPEED1000) != 0 {
                speed = SPEED_1000;
            } else if (bmcr & BMCR_SPEED100) != 0 {
                speed = SPEED_100;
            } else {
                speed = SPEED_10;
            }
            // AUTONEG_DISABLE
        }

        info!("DUPLEX: {}, SPEED: {}", duplex, speed);

        info!("Waiting for link ...");
        loop {
            // Read link status
            let status = self.mdio_read(phyaddr, MII_BMSR);
            link = status & BMSR_LSTATUS;

            if (link == BMSR_LSTATUS) {
                info!("Link is up! status: {:#x}", status);
                break;
            }
        }

        // 而没网线Link时, 不进行下列设置: PHY state change UP -> NOLINK
        if link != 0 {
            if pause != 0 {
                self.flow_ctrl(duplex, FLOW_CTRL, PAUSE);
            }

            self.set_link_mode(duplex, speed);
            // Link is Up
        }

        Ok(0)
        // 开始接收数据吧
    }

    pub fn can_recv(&mut self) -> bool {
        let desc = &self.recv_ring[self.rx_dirty];
        invalidate_dcache(
            virt_to_phys(desc as *const dma_desc as usize) as u64,
            size_of::<dma_desc>() as u64,
        );
        desc_get_own(desc) == 0
    }

    pub fn geth_recv(&mut self, limit: usize) -> (Vec<u8>, i32) {
        let mut rx_packets: u64 = 0;
        let mut rx_bytes: u64 = 0;
        let mut rxcount: usize = 0;
        let mut entry: usize = 0;
        let mut desc_count: usize = 0;
        let mut buffer: Vec<u8> = Vec::new();

        while rxcount < limit {
            entry = self.rx_dirty;
            let mut desc = &mut self.recv_ring[entry];

            invalidate_dcache(
                virt_to_phys(desc as *const dma_desc as usize) as u64,
                size_of::<dma_desc>() as u64,
            );

            if desc_get_own(desc) != 0 {
                break;
            }

            desc_count = entry;
            rxcount += 1;
            self.rx_dirty = (self.rx_dirty + 1) % DMA_DESC_RX;

            // Get length & status from hardware
            let mut frame_len = (desc.desc0 >> 16) & 0x3fff; // Frame length bit[16:29]

            //discard frame when last_desc, err_sum, len_err, mii_err
            let status = if (((desc.desc0 >> 8) & 0x1) == 0) || ((desc.desc0 & 0x9008) != 0) {
                rx_frame_status::discard_frame as i32
            } else {
                rx_frame_status::good_frame as i32
            };

            info!("RX frame size {}, status: {:?}", frame_len, status);

            if self.recv_buffers[entry] == 0 {
                error!("Recv buffer is NULL");
                break;
            }

            invalidate_dcache(
                virt_to_phys(self.recv_buffers[entry]) as u64,
                frame_len as u64,
            );

            let skb = unsafe {
                slice::from_raw_parts(self.recv_buffers[entry] as *const u8, frame_len as usize)
            };

            info!("========== RX PKT DATA: <<<<<<<<<<");
            print_hex_dump(skb, 64);

            if status == rx_frame_status::discard_frame as i32 {
                debug!("Get error packet");

                // Just need to clear 64 bits header
                unsafe {
                    slice::from_raw_parts_mut(self.recv_buffers[entry] as *mut u8, 64).fill(0);
                }
                flush_cache(virt_to_phys(self.recv_buffers[entry]) as u64, 64);

                continue;
            }

            if status != rx_frame_status::llc_snap as i32 {
                frame_len -= 4; // ETH_FCS_LEN, 帧出错检验
            }

            flush_cache(
                virt_to_phys(self.recv_buffers[entry]) as u64,
                frame_len as u64,
            );

            //注意只接收一个网络帧, limit=1
            buffer = unsafe {
                slice::from_raw_parts(self.recv_buffers[entry] as *const u8, frame_len as usize)
                    .to_vec()
            };

            /*
            // skb_put(skb, frame_len);
            // dma_unmap_single
            P::dealloc_dma(self.recv_buffers[entry], P::PAGE_SIZE);
            self.recv_buffers[entry] = 0;
            */

            info!(
                "desc_buf_get_addr: {:#x}, desc_buf_get_len: {}",
                desc.desc2,
                desc.desc1 & ((1 << 11) - 1)
            );

            //u-boot testing
            /*
            let paddr = desc.desc2 as u32;
            desc_buf_set(desc, paddr, MAX_BUF_SZ);
            desc_set_own(desc);
            */

            // eth_type_trans 包的协议分析

            rx_packets += 1;
            rx_bytes += frame_len as u64;
        }

        info!(
            "RX DMA State: {:#x}, recv packets: {}",
            read_volatile((self.base + GETH_RX_DMA_STA) as *mut u32),
            rx_packets
        );

        if rxcount > 0 {
            info!(
                "######### RX Descriptor DMA: {:#x}",
                self.recv_ring.as_ptr() as usize
            );
            info!(
                "RX pointor: dirty: {}, clean: {}",
                self.rx_dirty, self.rx_clean
            );
            info!(
                "[0]: {:#x?} \ndesc: {:#x?}",
                self.recv_ring[0], self.recv_ring[desc_count]
            );
        }

        self.rx_refill();

        (buffer, rxcount as i32)
    }

    pub fn can_send(&mut self) -> bool {
        let avail_tx = if self.tx_clean >= (self.tx_dirty + 1) {
            (self.tx_clean - (self.tx_dirty + 1))
        } else {
            DMA_DESC_TX - ((self.tx_dirty + 1) - self.tx_clean)
        };

        if avail_tx < 1 {
            error!("Tx Ring full !");
            return false;
        }
        /////////

        let desc = &self.send_ring[self.tx_dirty];
        invalidate_dcache(
            virt_to_phys(desc as *const dma_desc as usize) as u64,
            size_of::<dma_desc>() as u64,
        );
        if desc_get_own(desc) != 0 {
            return false;
        }

        let tx_status = read_volatile((self.base + GETH_TX_DMA_STA) as *mut u32) & 0b111;
        // from u-boot
        if (tx_status != 0b000) && (tx_status != 0b110) {
            return false;
        }

        true
    }

    pub fn geth_send(&mut self, send_buff: &[u8]) -> Result<i32, &str> {
        // Tx Ring full 判断一下？

        let mut entry = self.tx_dirty;
        //let mut first = &mut self.send_ring[entry];
        let first = entry;
        let mut desc = &mut self.send_ring[entry];
        let mut desc_count = entry;

        let csum_insert = 0; // 是否CHECKSUM_PARTIAL

        // linux驱动中的skb_headlen是什么?
        let mut len = send_buff.len() as u32;

        // send buffer长度需要注意下, 应该2k左右
        let target = unsafe {
            slice::from_raw_parts_mut(self.send_buffers[entry] as *mut u8, send_buff.len())
        };
        target.copy_from_slice(send_buff);

        if len > MAX_BUF_SZ {
            error!("The packet: {} to be send is TOO LARGE !", len);
        }

        info!("========== TX PKT DATA: >>>>>>>>>>");
        print_hex_dump(target, 64);

        while len != 0 {
            // 注意结构体所有权的问题
            desc = &mut self.send_ring[entry];
            desc_count = entry;

            let tmp_len = if len > MAX_BUF_SZ { MAX_BUF_SZ } else { len };
            // dma_map_single()
            // 当要发送的包 > MAX_BUF_SZ时，循环可能会出问题？

            let paddr = desc.desc2 as u32;
            desc_buf_set(desc, paddr, tmp_len);

            /* Don't set the first's own bit, here */
            // (first != desc)
            if (first != entry) {
                //self.send_buffers[entry] = 0;
                desc_set_own(desc);
            }

            entry = (entry + 1) % DMA_DESC_TX;
            len -= tmp_len;
        }

        // 例外情况处理nfrags. 多数情况等于0？

        self.tx_dirty = entry;
        // desc_tx_close(first, desc, csum_insert);
        self.desc_tx_close(first, desc_count, csum_insert);

        desc_set_own(&mut self.send_ring[first]);

        // 再判断下环形缓冲区的空间

        flush_cache(
            virt_to_phys(&self.send_ring[desc_count] as *const dma_desc as usize) as u64,
            size_of::<dma_desc>() as u64,
        );
        flush_cache(
            virt_to_phys(self.send_buffers[desc_count] as usize) as u64,
            send_buff.len() as u64,
        );

        info!(
            "######### TX Descriptor DMA: {:#x}",
            self.send_ring.as_ptr() as usize
        );
        info!(
            "TX pointor: dirty: {}, clean: {}",
            self.tx_dirty, self.tx_clean
        );
        info!(
            "[0]: {:#x?} \n[first]: {:#x?} \ndesc: {:#x?}",
            self.send_ring[0], self.send_ring[first], self.send_ring[desc_count]
        );

        info!(
            "TX DMA State: {:#x}",
            read_volatile((self.base + GETH_TX_DMA_STA) as *mut u32)
        );

        self.tx_poll();

        // 环形缓冲区的内存unmap之类的
        self.tx_complete();

        Ok(0)
    }

    pub fn rx_refill(&mut self) {
        while if self.rx_dirty >= (self.rx_clean + 1) {
            (self.rx_dirty - (self.rx_clean + 1))
        } else {
            DMA_DESC_RX - ((self.rx_clean + 1) - self.rx_dirty)
            // (self.rx_dirty - (self.rx_clean + 1)) & (DMA_DESC_RX - 1)
        } > 0
        {
            info!(
                "rx_refill, rx_dirty: {}, rx_clean: {}",
                self.rx_dirty, self.rx_clean
            );

            let entry = self.rx_clean;
            let mut desc = &mut self.recv_ring[entry];

            /* From Linux driver
            if self.recv_buffers[entry] == 0 {
                //申请socket buffer空间, 大小MAX_BUF_SZ, 2K左右
                // netdev_alloc_skb_ip_align
                // dma_map_single

                // desc_buf_set
            }
            */

            let paddr = desc.desc2 as u32;
            desc_buf_set(desc, paddr, MAX_BUF_SZ);
            desc_set_own(desc);
            flush_cache(
                virt_to_phys(&self.recv_ring[entry] as *const dma_desc as usize) as u64,
                size_of::<dma_desc>() as u64,
            );

            // sync memery
            fence_w();

            self.rx_clean = (self.rx_clean + 1) % DMA_DESC_RX;
        }
    }

    pub fn tx_complete(&mut self) {
        let mut entry = 0;
        let mut tx_stat = 0;
        let mut tx_packets: u64 = 0;
        let mut tx_errors: u64 = 0;

        while if self.tx_dirty >= self.tx_clean {
            (self.tx_dirty - self.tx_clean)
        } else {
            DMA_DESC_TX - (self.tx_clean - self.tx_dirty)
            //(self.tx_dirty - self.tx_clean) & (DMA_DESC_TX - 1)
        } > 0
        {
            debug!(
                "tx_complete, tx_dirty: {}, tx_clean: {}",
                self.tx_dirty, self.tx_clean
            );

            entry = self.tx_clean;
            let mut desc = &mut self.send_ring[entry];

            invalidate_dcache(
                virt_to_phys(desc as *const dma_desc as usize) as u64,
                size_of::<dma_desc>() as u64,
            );
            if desc_get_own(desc) != 0 {
                warn!("tx_complete get desc own failed !");
                break;
            }

            if desc_get_tx_ls(desc) != 0 {
                // Underflow error, No carrier, Loss of collision
                if (desc.desc0 & ((0b1 << 1) | (0b11 << 10))) != 0 {
                    tx_stat = -1;
                }

                if tx_stat == 0 {
                    tx_packets += 1;
                } else {
                    tx_errors += 1;
                }
            }

            // dma_unmap_single

            //self.send_buffers[entry], clear 2k
            unsafe {
                slice::from_raw_parts_mut(self.send_buffers[entry] as *mut u8, 2048).fill(0);
            }
            flush_cache(virt_to_phys(self.send_buffers[entry]) as u64, 2048);

            // 注意不要把desc2的Buffer Addr清零了
            desc_init(desc);
            self.tx_clean = (entry + 1) % DMA_DESC_TX;
        }

        debug!("send packets: {}, send errors: {}", tx_packets, tx_errors);
    }

    // Enable and Restart Autonegotiation
    pub fn phy_restart_aneg(&mut self) {
        // Don't isolate the PHY if we're negotiating
        self.phy_modify(MII_BMCR, BMCR_ISOLATE, BMCR_ANENABLE | BMCR_ANRESTART);

        info!("Enable and Restart Autonegotiation ...");
        // NOLINK --> autoneg_complete --> set speed and duplex --> LINK

        let phyaddr = 0;
        let mut autoneg_complete: u32 = 0;
        loop {
            // Read link and autonegotiation status
            let status = self.mdio_read(phyaddr, MII_BMSR);
            autoneg_complete = status & BMSR_ANEGCOMPLETE;
            //link = status & BMSR_LSTATUS;

            if autoneg_complete == BMSR_ANEGCOMPLETE {
                info!(
                    "Autonegotiation is completed ! autoneg_complete: {:#x}",
                    autoneg_complete
                );
                break;
            }
        }
    }

    pub fn phy_modify(&mut self, regnum: u32, mask: u32, set: u32) -> Result<i32, &str> {
        let phyaddr = 0;
        let ret: u32 = self.mdio_read(phyaddr, regnum);
        /*
        if ret < 0 {
            return Err("mdio read error !"); }
        */

        let new: u32 = (ret & !mask) | set;
        info!("phy_modify, read: {:#x}, set: {:#x}", ret, new);

        if new == ret {
            return Ok(0);
        }

        self.mdio_write(phyaddr, regnum, new);

        Ok(0)
    }

    pub fn power_on(&mut self) {
        let mut value: u32 = read_volatile((self.base_phy + EMAC_EPHY_CLK_REG0) as *mut u32);
        value &= !(1 << 15); // select EXT_PHY

        write_volatile((self.base_phy + EMAC_EPHY_CLK_REG0) as *mut u32, value);
    }

    pub fn clk_enable(&mut self) {
        // reset_control_deassert()
        // 注, clock未初始化好的话，mdio read phy无法读到有效数据
        self.deassert_emac_reset();

        // enable ephy clk
        let mut value: u32 = read_volatile((self.base_ccu + EMAC_25M_CLK_REG) as *mut u32);
        value |= 0b11 << 30;
        write_volatile((self.base_ccu + EMAC_25M_CLK_REG) as *mut u32, value);

        // clk_prepare_enable()

        let mut clk_value: u32 = read_volatile((self.base_phy + EMAC_EPHY_CLK_REG0) as *mut u32);
        info!("clk enable, Read PHY CLK: {:#x}", clk_value);
        // RGMII接口，支持10/100/1000 Mbps速率
        if self.phy_mode == RGMII {
            clk_value |= 0x00000004; // set RGMII
        } else {
            clk_value &= (!0x00000004);
        }

        clk_value &= (!0x00002003); // clear RMII_EN, ETCS

        if (self.phy_mode == RGMII) || (self.phy_mode == GMII) {
            clk_value |= 0x00000002; // set ETCS=2

        // RMII接口，支持10/100 Mbps速率
        } else if self.phy_mode == RMII {
            clk_value |= 0x00002001;
        }

        // Adjust Tx/Rx clock delay
        clk_value &= !(0x07 << 10);
        clk_value |= ((self.tx_delay & 0x07) << 10);
        clk_value &= !(0x1F << 5);
        clk_value |= ((self.rx_delay & 0x1F) << 5);

        info!("clk enable, write clk value: {:#x}", clk_value);
        write_volatile((self.base_phy + EMAC_EPHY_CLK_REG0) as *mut u32, clk_value);
    }

    pub fn deassert_emac_reset(&mut self) {
        let mut value: u32 = read_volatile((self.base_ccu + EMAC_BGR_REG) as *mut u32);
        info!("Read CCU value: {:#x}", value);
        value &= !(1 << 16); // assert reset
        write_volatile((self.base_ccu + EMAC_BGR_REG) as *mut u32, value);

        value |= (1 << 16); // deassert reset
        value |= 1; // enable bus clock
        write_volatile((self.base_ccu + EMAC_BGR_REG) as *mut u32, value);
    }

    pub fn interrupt_status(&mut self) -> i32 {
        // int status register
        let mut intr_status: u32 = read_volatile((self.base + GETH_RGMII_STA) as *mut u32);
        if (intr_status & RGMII_IRQ) != 0 {
            read_volatile((self.base + GETH_RGMII_STA) as *mut u32);
        }
        intr_status = read_volatile((self.base + GETH_INT_STA) as *mut u32);
        info!("interrupt_handle, GETH_INT_STA: {:#x}", intr_status);

        let mut status = 0;
        // 不正常的中断
        if (intr_status & TX_UNF_INT) != 0 {
            status = tx_dma_irq_status::tx_hard_error_bump_tc as i32;
        }
        if (intr_status & TX_STOP_INT) != 0 {
            status = tx_dma_irq_status::tx_hard_error as i32;
        }

        #[allow(clippy::collapsible_if)]
        /* 正常的 TX/RX NORMAL interrupts */
        if (intr_status & (TX_INT | RX_INT | RX_EARLY_INT | TX_UA_INT)) != 0
            && (intr_status & (TX_INT | RX_INT)) != 0
        {
            status = tx_dma_irq_status::handle_tx_rx as i32;
        }
        /* Clear the interrupt by writing a logic 1 to the CSR5[15-0] */
        write_volatile((self.base + GETH_INT_STA) as *mut u32, intr_status & 0x3FFF);

        status
    }

    pub fn interrupt_handle(&mut self, irq: u32, dev_id: &u32) -> Result<i32, &str> {
        let status = self.interrupt_status();

        // 处理
        if status == tx_dma_irq_status::handle_tx_rx as i32 {
            self.int_disable();
            // geth_poll()

            self.tx_complete(); // why? from Linux driver

            let (buffer, work_done) = self.geth_recv(BUDGET);
            if work_done < BUDGET as i32 {
                self.int_enable();
            }
        } else if status == tx_dma_irq_status::tx_hard_error as i32 {
            error!("gmac interrupt handle tx error !");
        } else {
            info!("gmac interrupt handle status: {}, Do nothing ...", status);
        }

        Ok(1)
    }

    pub fn int_enable(&mut self) {
        info!("Int enable");
        write_volatile((self.base + GETH_INT_EN) as *mut u32, RX_INT | TX_UNF_INT);
    }

    pub fn int_disable(&mut self) {
        info!("Int disable");
        write_volatile((self.base + GETH_INT_EN) as *mut u32, 0);
    }

    pub fn desc_tx_close(&mut self, first: usize, end: usize, csum_insert: usize) {
        let mut count = first;
        let mut desc = (&mut self.send_ring[first]) as *mut dma_desc;
        //let mut desc = first as *mut dma_desc;

        self.send_ring[first].desc1 |= 1 << 29; //First Segment,
        self.send_ring[end].desc1 |= 0b11 << 30; // Last Segment, Interrupt on completion

        if csum_insert != 0 {
            loop {
                unsafe {
                    (*desc).desc1 |= 0b11 << 27;
                    desc = desc.add(1);
                }
                count += 1;

                if count > end {
                    break;
                }
            }
        }
    }

    pub fn tx_poll(&self) {
        let value: u32 = read_volatile((self.base + GETH_TX_CTL1) as *mut u32);
        write_volatile((self.base + GETH_TX_CTL1) as *mut u32, value | 0x80000000);
    }

    pub fn rx_poll(&self) {
        let value: u32 = read_volatile((self.base + GETH_RX_CTL1) as *mut u32);
        write_volatile((self.base + GETH_RX_CTL1) as *mut u32, value | 0x80000000);
    }

    pub fn dma_init(&mut self) {
        write_volatile((self.base + GETH_BASIC_CTL1) as *mut u32, (8 << 24)); // burst
                                                                              // 打开网卡中断
        self.int_enable();
    }

    pub fn mac_reset(&mut self) -> Result<i32, &str> {
        let mut mac_reset_value: u32 = read_volatile((self.base + GETH_BASIC_CTL1) as *mut u32);
        mac_reset_value |= SOFT_RST;
        write_volatile((self.base + GETH_BASIC_CTL1) as *mut u32, mac_reset_value);

        // 原子上下文的等待
        //udelay(10000);
        while (SOFT_RST & read_volatile((self.base + GETH_BASIC_CTL1) as *mut u32)) != 0 {}

        let value = read_volatile((self.base + GETH_BASIC_CTL1) as *mut u32);
        info!("Read BASIC CTL1: {:#x}", value);
        if (value & SOFT_RST) == 0 {
            info!("Soft reset operation is completed !");
            Ok((value & SOFT_RST) as i32)
        } else {
            error!("Soft reset operation is NOT completed !");
            Err("mac Soft Reset failed !")
        }
    }

    pub fn mac_init(&mut self, txmode: usize, rxmode: usize) {
        self.dma_init();

        /* Initialize the core component */
        let mut value: u32 = read_volatile((self.base + GETH_TX_CTL0) as *mut u32);
        value |= (1 << 30); /* Jabber Disable */
        write_volatile((self.base + GETH_TX_CTL0) as *mut u32, value);
        info!("mac init, write TX_CTL0 {:#x}", value);

        let mut value: u32 = read_volatile((self.base + GETH_RX_CTL0) as *mut u32);
        value |= (1 << 27); /* Enable CRC & IPv4 Header Checksum */
        value |= (1 << 28); /* Automatic Pad/CRC Stripping */
        value |= (1 << 29); /* Jumbo Frame Enable */
        write_volatile((self.base + GETH_RX_CTL0) as *mut u32, value);
        info!("mac init, write RX_CTL0 {:#x}", value);

        write_volatile(
            (self.base + GETH_MDIO_ADDR) as *mut u32,
            (MDC_CLOCK_RATIO << 20),
        ); /* MDC_DIV_RATIO */

        /* Set the Rx&Tx mode */
        let mut value: u32 = read_volatile((self.base + GETH_TX_CTL1) as *mut u32);

        if txmode == SF_DMA_MODE {
            value |= TX_MD;
            value |= TX_NEXT_FRM;
        } else {
            value &= !TX_MD;
            value &= !TX_TH;
            /* Set the transmit threshold */
            if txmode <= 64 {
                value |= 0x00000000;
            } else if txmode <= 128 {
                value |= 0x00000100;
            } else if txmode <= 192 {
                value |= 0x00000200;
            } else {
                value |= 0x00000300;
            }
        }
        write_volatile((self.base + GETH_TX_CTL1) as *mut u32, value);
        info!("mac init, write TX_CTL1 {:#x}", value);

        let mut value: u32 = read_volatile((self.base + GETH_RX_CTL1) as *mut u32);
        // SF_DMA_MODE
        if rxmode == SF_DMA_MODE {
            value |= RX_MD;
        } else {
            value &= !RX_MD;
            value &= !RX_TH;
            if rxmode <= 32 {
                value |= 0x10;
            } else if rxmode <= 64 {
                value |= 0x00;
            } else if rxmode <= 96 {
                value |= 0x20;
            } else {
                value |= 0x30;
            }
        }
        /* Forward frames with error and undersized good frame. */
        value |= (RX_ERR_FRM | RX_RUNT_FRM);
        write_volatile((self.base + GETH_RX_CTL1) as *mut u32, value);
        info!("mac init, write RX_CTL1 {:#x}", value);
    }

    pub fn mac_enable(&mut self) {
        let mut value: u32 = read_volatile((self.base + GETH_TX_CTL0) as *mut u32);
        value |= (1 << 31);
        write_volatile((self.base + GETH_TX_CTL0) as *mut u32, value);

        let mut value: u32 = read_volatile((self.base + GETH_RX_CTL0) as *mut u32);
        value |= (1 << 31);
        write_volatile((self.base + GETH_RX_CTL0) as *mut u32, value);
    }

    pub fn mac_disable(&mut self) {
        let mut value: u32 = read_volatile((self.base + GETH_TX_CTL0) as *mut u32);
        value &= !(1 << 31);
        write_volatile((self.base + GETH_TX_CTL0) as *mut u32, value);

        let mut value: u32 = read_volatile((self.base + GETH_RX_CTL0) as *mut u32);
        value &= !(1 << 31);
        write_volatile((self.base + GETH_RX_CTL0) as *mut u32, value);
    }

    pub fn get_umac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn set_umac(&self, addr: &[u8; 6], index: u32) {
        info!(
            "Read mac addr high0 and low0: {:#x} {:#x}",
            read_volatile((self.base + GETH_ADDR_HI) as *mut u32),
            read_volatile((self.base + GETH_ADDR_LO) as *mut u32)
        );

        let data: u32 = ((addr[5] as u32) << 8) | (addr[4] as u32);
        write_volatile((self.base + GETH_ADDR_HI + (index << 3)) as *mut u32, data);
        let data: u32 = ((addr[3] as u32) << 24)
            | ((addr[2] as u32) << 16)
            | ((addr[1] as u32) << 8)
            | (addr[0] as u32);
        write_volatile((self.base + GETH_ADDR_LO + (index << 3)) as *mut u32, data);
    }

    pub fn start_rx(&mut self, rxbase: u32) {
        //rxbase需要32位对齐
        write_volatile((self.base + GETH_RX_DESC_LIST) as *mut u32, rxbase);

        let mut value: u32 = read_volatile((self.base + GETH_RX_CTL1) as *mut u32);
        value |= 0x40000000;
        write_volatile((self.base + GETH_RX_CTL1) as *mut u32, value);
    }

    pub fn stop_rx(&mut self) {
        let mut value: u32 = read_volatile((self.base + GETH_RX_CTL1) as *mut u32);
        value &= !0x40000000;
        write_volatile((self.base + GETH_RX_CTL1) as *mut u32, value);
    }

    pub fn start_tx(&mut self, txbase: u32) {
        //txbase需要32位对齐
        write_volatile((self.base + GETH_TX_DESC_LIST) as *mut u32, txbase);

        let mut value: u32 = read_volatile((self.base + GETH_TX_CTL1) as *mut u32);
        value |= 0x40000000;
        write_volatile((self.base + GETH_TX_CTL1) as *mut u32, value);
    }

    pub fn stop_tx(&mut self) {
        let mut value: u32 = read_volatile((self.base + GETH_TX_CTL1) as *mut u32);
        value &= !0x40000000;
        write_volatile((self.base + GETH_TX_CTL1) as *mut u32, value);
    }

    pub fn pinctrl_gpio_set(&mut self, offset: u32, value: u32) {
        // 0x0 <= offset <= 0x0350
        assert!(
            offset <= 0x0350,
            "Invalid gpio register offset: {:#x}",
            offset
        );

        let mut regval: u32 = read_volatile((self.pinctrl + offset) as *mut u32);
        //regval |= value;
        info!("GPIO offset: {:#x}, read regval: {:#x}", offset, regval);
        write_volatile((self.pinctrl + offset) as *mut u32, value);
    }

    pub fn hash_filter(&mut self, low: u32, high: u32) {
        info!("RX hash filter low: {:#x}, high: {:#x}", low, high);

        write_volatile((self.base + GETH_RX_HASH0) as *mut u32, high);
        write_volatile((self.base + GETH_RX_HASH1) as *mut u32, low);
    }

    pub fn set_filter(&mut self, flags: u64) {
        let mut tmp_flags: u32 = 0;

        tmp_flags |= ((flags >> 31)
            | ((flags >> 9) & 0x00000002)
            | ((flags << 1) & 0x00000010)
            | ((flags >> 3) & 0x00000060)
            | ((flags << 7) & 0x00000300)
            | ((flags << 6) & 0x00003000)
            | ((flags << 12) & 0x00030000)
            | (flags << 31)) as u32;

        info!(
            "Set RX frame filter, flags: {:#x}, write value: {:#x}",
            flags, tmp_flags
        );
        write_volatile((self.base + GETH_RX_FRM_FLT) as *mut u32, tmp_flags);
    }

    pub fn set_link_mode(&mut self, duplex: i32, speed: i32) {
        let mut ctrl: u32 = read_volatile((self.base + GETH_BASIC_CTL0) as *mut u32);
        if duplex == 0 {
            ctrl &= !(CTL0_DM);
        } else {
            ctrl |= CTL0_DM;
        }

        match speed {
            1000 => ctrl &= !0x0C,
            100 | 10 => {
                ctrl |= 0x08;
                if (speed == 100) {
                    ctrl |= 0x04;
                } else {
                    ctrl &= !0x04;
                }
            }
            _ => {
                ctrl |= 0x08;
                if (speed == 100) {
                    ctrl |= 0x04;
                } else {
                    ctrl &= !0x04;
                }
            }
        }

        write_volatile((self.base + GETH_BASIC_CTL0) as *mut u32, ctrl);

        let value = read_volatile((self.base + GETH_BASIC_CTL0) as *mut u32);
        info!(
            "Set link mode:  duplex {}, speed {}, CTL0: {:#x}",
            duplex, speed, value
        );
    }

    pub fn mac_loopback(&mut self, enable: u32) {
        let mut reg: u32 = read_volatile((self.base + GETH_BASIC_CTL0) as *mut u32);
        if enable != 0 {
            reg |= 0x02;
        } else {
            reg &= !0x02;
        }
        write_volatile((self.base + GETH_BASIC_CTL0) as *mut u32, reg);
    }

    pub fn flow_ctrl(&mut self, duplex: i32, fc: u32, pause: u32) {
        let mut flow: u32 = 0;
        info!(
            "Set flow ctrl: duplex {}, fc {}, pause {}",
            duplex, fc, pause
        );

        if fc & FLOW_RX != 0 {
            flow = read_volatile((self.base + GETH_RX_CTL0) as *mut u32);
            flow |= 0x10000;
            write_volatile((self.base + GETH_RX_CTL0) as *mut u32, flow);
        }

        if fc & FLOW_TX != 0 {
            flow = read_volatile((self.base + GETH_TX_FLOW_CTL) as *mut u32);
            flow |= 0x00001;
            write_volatile((self.base + GETH_TX_FLOW_CTL) as *mut u32, flow);
        }

        if duplex != 0 {
            flow = read_volatile((self.base + GETH_TX_FLOW_CTL) as *mut u32);
            flow |= (pause << 4);
            write_volatile((self.base + GETH_TX_FLOW_CTL) as *mut u32, flow);
        }
    }

    pub fn mdio_read(&mut self, phyaddr: u32, phyreg: u32) -> u32 {
        let mut value: u32 = 0;

        value |= ((MDC_CLOCK_RATIO & 0x07) << 20);

        value |= (((phyaddr << 12) & (0x0001F000)) | ((phyreg << 4) & (0x000007F0)) | MII_BUSY);

        while (read_volatile((self.base + GETH_MDIO_ADDR) as *mut u32) & MII_BUSY) == 1 {}

        write_volatile((self.base + GETH_MDIO_ADDR) as *mut u32, value);

        while (read_volatile((self.base + GETH_MDIO_ADDR) as *mut u32) & MII_BUSY) == 1 {}

        //16位有效
        let ret = read_volatile((self.base + GETH_MDIO_DATA) as *mut u32);
        // info!("mdio_read MDIO DATA: {:#x}", ret);

        ret as u32
    }

    pub fn mdio_write(&mut self, phyaddr: u32, phyreg: u32, data: u32) {
        let mut value: u32 = ((0x07 << 20)
            & read_volatile((self.base + GETH_MDIO_ADDR) as *mut u32)
            | (MDC_CLOCK_RATIO << 20));

        value |= (((phyaddr << 12) & (0x0001F000)) | ((phyreg << 4) & (0x000007F0)))
            | MII_WRITE
            | MII_BUSY;

        while (read_volatile((self.base + GETH_MDIO_ADDR) as *mut u32) & MII_BUSY) == 1 {}

        write_volatile((self.base + GETH_MDIO_DATA) as *mut u32, data);
        write_volatile((self.base + GETH_MDIO_ADDR) as *mut u32, value);

        while (read_volatile((self.base + GETH_MDIO_ADDR) as *mut u32) & MII_BUSY) == 1 {}
    }

    pub fn mdio_reset(&mut self) {
        write_volatile((self.base + GETH_MDIO_ADDR) as *mut u32, (4 << 2));
    }
}

pub fn desc_set_own(desc: &mut dma_desc) {
    desc.desc0 |= 0x80000000;
}

pub fn desc_get_own(desc: &dma_desc) -> u32 {
    desc.desc0 & 0x80000000
}

pub fn desc_get_tx_ls(desc: &dma_desc) -> u32 {
    desc.desc1 & 0x40000000 // Last Segment
}

pub fn desc_buf_set(desc: &mut dma_desc, paddr: u32, size: u32) {
    desc.desc1 &= (!((1 << 11) - 1));
    desc.desc1 |= (size & ((1 << 11) - 1));
    desc.desc2 = paddr;
}

pub fn desc_init(desc: &mut dma_desc) {
    desc.desc1 = 0;
    desc.desc1 |= (1 << 24);

    // 这里用的Buffer Addr不发生改变
    //desc.desc2 = 0;
}

fn read_volatile<T>(src: *const T) -> T {
    unsafe { core::ptr::read_volatile(phys_to_virt(src as usize) as *const T) }
}

fn write_volatile<T>(dst: *mut T, value: T) {
    unsafe {
        core::ptr::write_volatile(phys_to_virt(dst as usize) as *mut T, value);
    }
}

pub fn print_hex_dump(buf: &[u8], len: usize) {}
