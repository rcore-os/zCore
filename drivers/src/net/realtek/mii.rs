// From Linux

/* Generic MII registers. */
pub const MII_BMCR: u32 = 0x00;
pub const MII_BMSR: u32 = 0x01;
pub const MII_PHYSID1: u32 = 0x02;
pub const MII_PHYSID2: u32 = 0x03;
pub const MII_ADVERTISE: u32 = 0x04;
pub const MII_LPA: u32 = 0x05;
pub const MII_EXPANSION: u32 = 0x06;
pub const MII_CTRL1000: u32 = 0x09;
pub const MII_STAT1000: u32 = 0x0a;
pub const MII_MMD_CTRL: u32 = 0x0d;
pub const MII_MMD_DATA: u32 = 0x0e;
pub const MII_ESTATUS: u32 = 0x0f;
pub const MII_DCOUNTER: u32 = 0x12;
pub const MII_FCSCOUNTER: u32 = 0x13;
pub const MII_NWAYTEST: u32 = 0x14;
pub const MII_RERRCOUNTER: u32 = 0x15;
pub const MII_SREVISION: u32 = 0x16;
pub const MII_RESV1: u32 = 0x17;
pub const MII_LBRERROR: u32 = 0x18;
pub const MII_PHYADDR: u32 = 0x19;
pub const MII_RESV2: u32 = 0x1a;
pub const MII_TPISTATUS: u32 = 0x1b;
pub const MII_NCONFIG: u32 = 0x1c;

/* Basic mode control register. */
pub const BMCR_RESV: u32 = 0x003f;
pub const BMCR_SPEED1000: u32 = 0x0040;
pub const BMCR_CTST: u32 = 0x0080;
pub const BMCR_FULLDPLX: u32 = 0x0100;
pub const BMCR_ANRESTART: u32 = 0x0200;
pub const BMCR_ISOLATE: u32 = 0x0400;
pub const BMCR_PDOWN: u32 = 0x0800;
pub const BMCR_ANENABLE: u32 = 0x1000;
pub const BMCR_SPEED100: u32 = 0x2000;
pub const BMCR_LOOPBACK: u32 = 0x4000;
pub const BMCR_RESET: u32 = 0x8000;
pub const BMCR_SPEED10: u32 = 0x0000;

/* Basic mode status register. */
pub const BMSR_ERCAP: u32 = 0x0001;
pub const BMSR_JCD: u32 = 0x0002;
pub const BMSR_LSTATUS: u32 = 0x0004;
pub const BMSR_ANEGCAPABLE: u32 = 0x0008;
pub const BMSR_RFAULT: u32 = 0x0010;
pub const BMSR_ANEGCOMPLETE: u32 = 0x0020;
pub const BMSR_RESV: u32 = 0x00c0;
pub const BMSR_ESTATEN: u32 = 0x0100;
pub const BMSR_100HALF2: u32 = 0x0200;
pub const BMSR_100FULL2: u32 = 0x0400;
pub const BMSR_10HALF: u32 = 0x0800;
pub const BMSR_10FULL: u32 = 0x1000;
pub const BMSR_100HALF: u32 = 0x2000;
pub const BMSR_100FULL: u32 = 0x4000;
pub const BMSR_100BASE4: u32 = 0x8000;

/* Advertisement control register. */
pub const ADVERTISE_SLCT: u32 = 0x001f;
pub const ADVERTISE_CSMA: u32 = 0x0001;
pub const ADVERTISE_10HALF: u32 = 0x0020;
pub const ADVERTISE_1000XFULL: u32 = 0x0020;
pub const ADVERTISE_10FULL: u32 = 0x0040;
pub const ADVERTISE_1000XHALF: u32 = 0x0040;
pub const ADVERTISE_100HALF: u32 = 0x0080;
pub const ADVERTISE_1000XPAUSE: u32 = 0x0080;
pub const ADVERTISE_100FULL: u32 = 0x0100;
pub const ADVERTISE_1000XPSE_ASYM: u32 = 0x0100;
pub const ADVERTISE_100BASE4: u32 = 0x0200;
pub const ADVERTISE_PAUSE_CAP: u32 = 0x0400;
pub const ADVERTISE_PAUSE_ASYM: u32 = 0x0800;
pub const ADVERTISE_RESV: u32 = 0x1000;
pub const ADVERTISE_RFAULT: u32 = 0x2000;
pub const ADVERTISE_LPACK: u32 = 0x4000;
pub const ADVERTISE_NPAGE: u32 = 0x8000;

pub const ADVERTISE_FULL: u32 = ADVERTISE_100FULL | ADVERTISE_10FULL | ADVERTISE_CSMA;
pub const ADVERTISE_ALL: u32 =
    ADVERTISE_10HALF | ADVERTISE_10FULL | ADVERTISE_100HALF | ADVERTISE_100FULL;

/* Link partner ability register. */
pub const LPA_SLCT: u32 = 0x001f;
pub const LPA_10HALF: u32 = 0x0020;
pub const LPA_1000XFULL: u32 = 0x0020;
pub const LPA_10FULL: u32 = 0x0040;
pub const LPA_1000XHALF: u32 = 0x0040;
pub const LPA_100HALF: u32 = 0x0080;
pub const LPA_1000XPAUSE: u32 = 0x0080;
pub const LPA_100FULL: u32 = 0x0100;
pub const LPA_1000XPAUSE_ASYM: u32 = 0x0100;
pub const LPA_100BASE4: u32 = 0x0200;
pub const LPA_PAUSE_CAP: u32 = 0x0400;
pub const LPA_PAUSE_ASYM: u32 = 0x0800;
pub const LPA_RESV: u32 = 0x1000;
pub const LPA_RFAULT: u32 = 0x2000;
pub const LPA_LPACK: u32 = 0x4000;
pub const LPA_NPAGE: u32 = 0x8000;

pub const LPA_DUPLEX: u32 = (LPA_10FULL | LPA_100FULL);
pub const LPA_100: u32 = (LPA_100FULL | LPA_100HALF | LPA_100BASE4);

/* 1000BASE-T Control register */
pub const ADVERTISE_1000FULL: u32 = 0x0200;
pub const ADVERTISE_1000HALF: u32 = 0x0100;
pub const CTL1000_AS_MASTER: u32 = 0x0800;
pub const CTL1000_ENABLE_MASTER: u32 = 0x1000;

/* 1000BASE-T Status register */
pub const LPA_1000MSFAIL: u32 = 0x8000;
pub const LPA_1000LOCALRXOK: u32 = 0x2000;
pub const LPA_1000REMRXOK: u32 = 0x1000;
pub const LPA_1000FULL: u32 = 0x0800;
pub const LPA_1000HALF: u32 = 0x0400;

/* Flow control flags */
pub const FLOW_CTRL_TX: u32 = 0x01;
pub const FLOW_CTRL_RX: u32 = 0x02;
