#![allow(missing_docs)]
//! Port packet data structure definition.

use super::*;
use core::fmt::{Debug, Formatter};

// C struct: for storing

/// A packet sent through a port.
#[repr(C)]
pub struct PortPacket {
    pub key: u64,
    pub type_: PacketType,
    pub status: ZxError,
    pub data: Payload,
}

// reference: zircon/system/public/zircon/syscalls/port.h ZX_PKT_TYPE_*
/// The type of a packet.
#[repr(u32)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum PacketType {
    User = 0,
    SignalOne = 1,
    SignalRep = 2,
    GuestBell = 3,
    GuestMem = 4,
    GuestIo = 5,
    GuestVcpu = 6,
    Interrupt = 7,
    PageRequest = 9,
}

#[repr(C)]
/// The data carried by a packet
pub union Payload {
    user: PacketUser,
    signal: PacketSignal,
    guest_bell: PacketGuestBell,
    guest_mem: PacketGuestMem,
    guest_io: PacketGuestIo,
    guest_vcpu: PacketGuestVcpu,
    interrupt: PacketInterrupt,
    // TODO: PacketPageRequest
}

pub type PacketUser = [u8; 32];

#[repr(C)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct PacketSignal {
    pub trigger: Signal,
    pub observed: Signal,
    pub count: u64,
    pub timestamp: u64,
    pub _reserved1: u64,
}

#[repr(C)]
#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub struct PacketGuestBell {
    pub addr: u64,
    pub _reserved0: u64,
    pub _reserved1: u64,
    pub _reserved2: u64,
}

#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub struct PacketGuestMem {
    pub addr: u64,
    pub inst_len: u8,
    pub inst_buf: [u8; 15],
    pub default_operand_size: u8,
    pub _reserved: [u8; 7],
}

#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub struct PacketGuestMem {
    pub addr: u64,
    pub access_size: u8,
    pub sign_extend: bool,
    pub xt: u8,
    pub read: bool,
    pub _padding1: [u8; 4],
    pub data: u64,
    pub _reserved: u64,
}

#[repr(C)]
#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub struct PacketGuestIo {
    pub port: u16,
    pub access_size: u8,
    pub input: bool,
    pub data: [u8; 4],
    pub _reserved0: u64,
    pub _reserved1: u64,
    pub _reserved2: u64,
}

#[repr(u8)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum PacketGuestVcpuType {
    VcpuInterrupt = 0,
    VcpuStartup = 1,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union PacketGuestVcpuData {
    interrupt: PacketGuestVcpuInterrupt,
    startup: PacketGuestVcpuStartup,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct PacketGuestVcpuInterrupt {
    mask: u64,
    vector: u8,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct PacketGuestVcpuStartup {
    id: u64,
    entry: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct PacketGuestVcpu {
    pub data: PacketGuestVcpuData,
    pub type_: PacketGuestVcpuType,
    pub _padding1: [u8; 7],
    pub _reserved: u64,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct PacketInterrupt {
    pub timestamp: i64,
    pub _reserved0: u64,
    pub _reserved1: u64,
    pub _reserved2: u64,
}

// Rust struct: for internal constructing and debugging

/// A high-level representation of a packet sent through a port.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PortPacketRepr {
    pub key: u64,
    pub status: ZxError,
    pub data: PayloadRepr,
}

/// A high-level representation of a packet payload.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PayloadRepr {
    User(PacketUser),
    Signal(PacketSignal),
    GuestBell(PacketGuestBell),
    GuestMem(PacketGuestMem),
    GuestIo(PacketGuestIo),
    GuestVcpu(PacketGuestVcpu),
    Interrupt(PacketInterrupt),
}

impl PayloadRepr {
    fn type_(&self) -> PacketType {
        match self {
            PayloadRepr::User(_) => PacketType::User,
            PayloadRepr::Signal(_) => PacketType::SignalOne,
            PayloadRepr::GuestBell(_) => PacketType::GuestBell,
            PayloadRepr::GuestMem(_) => PacketType::GuestMem,
            PayloadRepr::GuestIo(_) => PacketType::GuestIo,
            PayloadRepr::GuestVcpu(_) => PacketType::GuestVcpu,
            PayloadRepr::Interrupt(_) => PacketType::Interrupt,
        }
    }
    fn encode(&self) -> Payload {
        match *self {
            PayloadRepr::User(user) => Payload { user },
            PayloadRepr::Signal(signal) => Payload { signal },
            PayloadRepr::GuestBell(guest_bell) => Payload { guest_bell },
            PayloadRepr::GuestMem(guest_mem) => Payload { guest_mem },
            PayloadRepr::GuestIo(guest_io) => Payload { guest_io },
            PayloadRepr::GuestVcpu(guest_vcpu) => Payload { guest_vcpu },
            PayloadRepr::Interrupt(interrupt) => Payload { interrupt },
        }
    }
    #[allow(unsafe_code)]
    fn decode(type_: PacketType, data: &Payload) -> Self {
        unsafe {
            match type_ {
                PacketType::User => PayloadRepr::User(data.user),
                PacketType::SignalOne => PayloadRepr::Signal(data.signal),
                PacketType::SignalRep => PayloadRepr::Signal(data.signal),
                PacketType::GuestBell => PayloadRepr::GuestBell(data.guest_bell),
                PacketType::GuestMem => PayloadRepr::GuestMem(data.guest_mem),
                PacketType::GuestIo => PayloadRepr::GuestIo(data.guest_io),
                PacketType::GuestVcpu => PayloadRepr::GuestVcpu(data.guest_vcpu),
                PacketType::Interrupt => PayloadRepr::Interrupt(data.interrupt),
                _ => unimplemented!(),
            }
        }
    }
}

impl From<PortPacketRepr> for PortPacket {
    fn from(r: PortPacketRepr) -> Self {
        PortPacket {
            key: r.key,
            type_: r.data.type_(),
            status: r.status,
            data: r.data.encode(),
        }
    }
}

impl From<&PortPacket> for PortPacketRepr {
    fn from(p: &PortPacket) -> Self {
        PortPacketRepr {
            key: p.key,
            status: p.status,
            data: PayloadRepr::decode(p.type_, &p.data),
        }
    }
}

impl PartialEq for PacketGuestVcpu {
    #[allow(unsafe_code)]
    fn eq(&self, other: &Self) -> bool {
        if !self.type_.eq(&other.type_)
            || !self._padding1.eq(&other._padding1)
            || !self._reserved.eq(&other._reserved)
        {
            return false;
        }
        unsafe {
            match self.type_ {
                PacketGuestVcpuType::VcpuInterrupt => self.data.interrupt.eq(&other.data.interrupt),
                PacketGuestVcpuType::VcpuStartup => self.data.startup.eq(&other.data.startup),
            }
        }
    }
}

impl Eq for PacketGuestVcpu {}

impl Debug for PacketGuestVcpu {
    #[allow(unsafe_code)]
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        let mut out = f.debug_struct("PacketGuestVcpu");
        unsafe {
            match self.type_ {
                PacketGuestVcpuType::VcpuInterrupt => out.field("data", &self.data.interrupt),
                PacketGuestVcpuType::VcpuStartup => out.field("data", &self.data.startup),
            };
        }
        out.field("type_", &self.type_)
            .field("_padding1", &self._padding1)
            .field("_reserved", &self._reserved)
            .finish()
    }
}

impl Debug for PortPacket {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        PortPacketRepr::from(self).fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_packet_size() {
        use core::mem::size_of;
        assert_eq!(size_of::<PacketUser>(), 32);
        assert_eq!(size_of::<PacketSignal>(), 32);
        assert_eq!(size_of::<PacketGuestBell>(), 32);
        assert_eq!(size_of::<PacketGuestMem>(), 32);
        assert_eq!(size_of::<PacketGuestIo>(), 32);
        assert_eq!(size_of::<PacketGuestVcpu>(), 32);
        assert_eq!(size_of::<PacketInterrupt>(), 32);
    }

    fn test_encdec(data: PayloadRepr) {
        let repr = PortPacketRepr {
            key: 0,
            status: ZxError::OK,
            data: data.clone(),
        };
        let packet = PortPacket {
            key: 0,
            type_: data.type_(),
            status: ZxError::OK,
            data: data.encode(),
        };
        assert_eq!(repr, PortPacketRepr::from(&packet));
        assert_eq!(repr.clone(), PortPacketRepr::from(&PortPacket::from(repr)));
    }

    #[test]
    fn user() {
        let user = PacketUser::default();
        test_encdec(PayloadRepr::User(user));
    }

    #[test]
    fn signal() {
        let data = PacketSignal {
            trigger: Signal::READABLE,
            observed: Signal::WRITABLE,
            count: 1,
            timestamp: 0,
            _reserved1: 0,
        };
        test_encdec(PayloadRepr::Signal(data));
        let packet = PortPacket {
            key: 1,
            type_: PacketType::SignalOne,
            status: ZxError::OK,
            data: Payload { signal: data },
        };
        let packet_ = PortPacket {
            key: 1,
            type_: PacketType::SignalRep,
            status: ZxError::OK,
            data: Payload { signal: data },
        };
        assert_eq!(
            PortPacketRepr::from(&packet),
            PortPacketRepr::from(&packet_)
        );
    }

    #[test]
    fn guest_bell() {
        let guest_bell = PacketGuestBell::default();
        assert_eq!(guest_bell.addr, 0);
        test_encdec(PayloadRepr::GuestBell(guest_bell));
    }

    #[test]
    fn guest_mem() {
        let guest_mem = PacketGuestMem::default();
        assert_eq!(guest_mem.addr, 0);
        test_encdec(PayloadRepr::GuestMem(guest_mem));
    }

    #[test]
    fn guest_io() {
        let guest_io = PacketGuestIo::default();
        assert_eq!(guest_io.port, 0);
        assert_eq!(guest_io.input, false);
        test_encdec(PayloadRepr::GuestIo(guest_io));
    }

    #[test]
    fn guest_vcpu() {
        let interrupt = PacketGuestVcpuInterrupt { mask: 0, vector: 0 };
        let guest_vcpu1 = PacketGuestVcpu {
            data: PacketGuestVcpuData { interrupt },
            type_: PacketGuestVcpuType::VcpuInterrupt,
            _padding1: Default::default(),
            _reserved: 0,
        };
        let startup = PacketGuestVcpuStartup { id: 0, entry: 0 };
        let guest_vcpu2 = PacketGuestVcpu {
            data: PacketGuestVcpuData { startup },
            type_: PacketGuestVcpuType::VcpuStartup,
            _padding1: Default::default(),
            _reserved: 0,
        };
        test_encdec(PayloadRepr::GuestVcpu(guest_vcpu1));
        test_encdec(PayloadRepr::GuestVcpu(guest_vcpu2));

        let packet = PortPacket {
            key: 1,
            type_: PacketType::GuestVcpu,
            status: ZxError::OK,
            data: Payload {
                guest_vcpu: guest_vcpu2,
            },
        };
        assert_eq!(
            format!("{:?}", packet),
            "PortPacketRepr { key: 1, status: OK, data: GuestVcpu(PacketGuestVcpu { data: PacketGuestVcpuStartup { id: 0, entry: 0 }, type_: VcpuStartup, _padding1: [0, 0, 0, 0, 0, 0, 0], _reserved: 0 }) }"
        );

        assert!(!guest_vcpu1.eq(&guest_vcpu2));
        let guest_vcpu3 = PacketGuestVcpu {
            data: PacketGuestVcpuData {
                startup: PacketGuestVcpuStartup { id: 0, entry: 1 },
            },
            type_: PacketGuestVcpuType::VcpuStartup,
            _padding1: Default::default(),
            _reserved: 0,
        };
        assert!(!guest_vcpu2.eq(&guest_vcpu3));
    }

    #[test]
    fn interrupt() {
        let interrupt = PacketInterrupt {
            timestamp: 12345,
            _reserved0: 0,
            _reserved1: 0,
            _reserved2: 0,
        };
        test_encdec(PayloadRepr::Interrupt(interrupt));
    }

    #[test]
    #[should_panic(expected = "not implemented")]
    fn page_request() {
        let data: PacketUser = [0u8; 32];
        PayloadRepr::decode(PacketType::PageRequest, &Payload { user: data });
    }
}
