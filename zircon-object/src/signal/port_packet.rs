//! Port packet data structure definition.

use super::*;
use core::fmt::{Debug, Formatter};

// C struct: for storing

/// A packet sent through a port.
#[repr(C)]
pub struct PortPacket {
    pub key: u64,
    pub type_: PacketType,
    exception_num: u8,
    _padding: u16,
    pub status: ZxError,
    pub data: Payload,
}

// reference: zircon/system/public/zircon/syscalls/port.h ZX_PKT_TYPE_*
#[repr(u8)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[allow(dead_code)]
pub enum PacketType {
    User = 0,
    SignalOne = 1,
    SignalRep = 2,
    GuestBell = 3,
    GuestMem = 4,
    GuestIo = 5,
    GuestVcpu = 6,
    Interrupt = 7,
    Exception = 8,
    PageRequest = 9,
}

#[repr(C)]
pub union Payload {
    signal: PacketSignal,
    exception: PacketException,
    interrupt: PacketInterrupt,
    user: [u8; 32],
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct PacketSignal {
    pub trigger: Signal,
    pub observed: Signal,
    pub count: u64,
    pub timestamp: u64,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct PacketException {
    pub pid: KoID,
    pub tid: KoID,
    pub num: u8,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct PacketInterrupt {
    pub timestamp: i64,
    pub reserved0: u64,
    pub reserved1: u64,
    pub reserved2: u64,
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
    Signal(PacketSignal),
    Exception(PacketException),
    Interrupt(PacketInterrupt),
    User([u8; 32]),
}

impl PayloadRepr {
    fn exception_num(&self) -> u8 {
        match self {
            PayloadRepr::Exception(exception) => exception.num,
            _ => 0,
        }
    }
    fn type_(&self) -> PacketType {
        match self {
            PayloadRepr::User(_) => PacketType::User,
            PayloadRepr::Signal(_) => PacketType::SignalOne,
            PayloadRepr::Exception(_) => PacketType::Exception,
            PayloadRepr::Interrupt(_) => PacketType::Interrupt,
        }
    }
    fn encode(&self) -> Payload {
        match *self {
            PayloadRepr::Signal(signal) => Payload { signal },
            PayloadRepr::Exception(exception) => Payload { exception },
            PayloadRepr::Interrupt(interrupt) => Payload { interrupt },
            PayloadRepr::User(user) => Payload { user },
        }
    }
    #[allow(unsafe_code)]
    fn decode(type_: PacketType, exception_num: u8, data: &Payload) -> Self {
        unsafe {
            match type_ {
                PacketType::User => PayloadRepr::User(data.user),
                PacketType::SignalOne => PayloadRepr::Signal(data.signal),
                PacketType::SignalRep => PayloadRepr::Signal(data.signal),
                PacketType::Exception => PayloadRepr::Exception(PacketException {
                    num: exception_num,
                    ..data.exception
                }),
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
            exception_num: r.data.exception_num(),
            _padding: 0,
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
            data: PayloadRepr::decode(p.type_, p.exception_num, &p.data),
        }
    }
}

impl Debug for PortPacket {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        PortPacketRepr::from(self).fmt(f)
    }
}
