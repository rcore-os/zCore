//! ICMP/ICMPv6 echo replies delivered from RX frames (same path as DHCP via `push_packet`).
//! smoltcp ingress is not relied on for ping RX.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use kernel_hal::net::get_net_device;
use lazy_static::lazy_static;
use lock::Mutex;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    Icmpv4Packet, Icmpv6Packet, Icmpv6Repr, IpAddress, IpCidr, IpProtocol, Ipv4Address, Ipv4Packet,
    Ipv6Packet,
};
/// Pre-DHCP sentinel still present in older images; not a usable host address.
pub fn is_ipv4_placeholder(addr: Ipv4Address) -> bool {
    addr == Ipv4Address::new(240, 0, 0, 0)
}

const RX_QUEUE_MAX: usize = 64;
const MAX_ICMP_PAYLOAD: usize = 1280;

struct IcmpRxPacket {
    src: IpAddress,
    data: Vec<u8>,
}

lazy_static! {
    static ref RX_QUEUE: Mutex<VecDeque<IcmpRxPacket>> = Mutex::new(VecDeque::new());
}

fn is_our_ip(addr: IpAddress) -> bool {
    if addr.is_unspecified() {
        return false;
    }
    match addr {
        IpAddress::Ipv4(a) if a.is_loopback() => return true,
        IpAddress::Ipv6(a) if a.is_loopback() => return true,
        _ => {}
    }
    for dev in get_net_device().iter() {
        for ip in dev.get_ip_address() {
            match (ip, addr) {
                (IpCidr::Ipv4(cidr), IpAddress::Ipv4(a))
                    if !is_ipv4_placeholder(cidr.address())
                        && cidr.prefix_len() > 0
                        && cidr.address() == a =>
                {
                    return true;
                }
                (IpCidr::Ipv6(cidr), IpAddress::Ipv6(a))
                    if cidr.prefix_len() > 0 && cidr.address() == a =>
                {
                    return true;
                }
                _ => {}
            }
        }
    }
    false
}

/// Parse an Ethernet frame; queue ICMP / ICMPv6 echo replies addressed to us.
pub fn deliver_from_frame(frame: &[u8]) {
    if frame.len() < 14 {
        return;
    }
    let mut l2 = 14usize;
    let mut et = u16::from_be_bytes([frame[12], frame[13]]);
    if et == 0x8100 {
        if frame.len() < 18 {
            return;
        }
        l2 = 18;
        et = u16::from_be_bytes([frame[16], frame[17]]);
    }
    if et == 0x0800 {
        let ip = &frame[l2..];
        let Ok(pkt) = Ipv4Packet::new_checked(ip) else {
            return;
        };
        if pkt.protocol() != IpProtocol::Icmp {
            return;
        }
        let src = IpAddress::Ipv4(pkt.src_addr());
        let dst = IpAddress::Ipv4(pkt.dst_addr());
        if !is_our_ip(dst) {
            return;
        }
        let payload = pkt.payload();
        if payload.is_empty() {
            return;
        }
        if payload[0] != 0 {
            // 0 = Echo Reply
            return;
        }
        // Verify the ICMPv4 checksum to reject corrupt/truncated frames.
        let Ok(icmp_pkt) = Icmpv4Packet::new_checked(payload) else {
            return;
        };
        if !icmp_pkt.verify_checksum() {
            return;
        }
        let mut q = RX_QUEUE.lock();
        if q.len() >= RX_QUEUE_MAX {
            q.pop_front();
        }
        q.push_back(IcmpRxPacket {
            src,
            data: payload[..payload.len().min(MAX_ICMP_PAYLOAD)].to_vec(),
        });
        drop(q);
        kernel_hal::net::wake_net_rx_waiters();
    } else if et == 0x86dd {
        let ip = &frame[l2..];
        let Ok(pkt) = Ipv6Packet::new_checked(ip) else {
            return;
        };
        if pkt.next_header() != IpProtocol::Icmpv6 {
            return;
        }
        let Ok(icmp_pkt) = smoltcp::wire::Icmpv6Packet::new_checked(pkt.payload()) else {
            return;
        };
        let src = IpAddress::Ipv6(pkt.src_addr());
        let dst = IpAddress::Ipv6(pkt.dst_addr());
        if !icmp_pkt.verify_checksum(&src, &dst) {
            return;
        }
        if !is_our_ip(dst) {
            return;
        }
        let payload = pkt.payload();
        if payload.is_empty() {
            return;
        }
        if payload[0] != 129 {
            // 129 = Echo Reply
            return;
        }
        let mut q = RX_QUEUE.lock();
        if q.len() >= RX_QUEUE_MAX {
            q.pop_front();
        }
        q.push_back(IcmpRxPacket {
            src,
            data: payload[..payload.len().min(MAX_ICMP_PAYLOAD)].to_vec(),
        });
        drop(q);
        kernel_hal::net::wake_net_rx_waiters();
    }
}

fn finalize_icmp_echo_reply_v4(icmp: &mut [u8]) {
    if icmp.len() < 8 {
        return;
    }
    if icmp[0] == 8 {
        icmp[0] = 0;
    }
    let mut pkt = Icmpv4Packet::new_unchecked(icmp);
    pkt.fill_checksum();
}

fn finalize_icmp_echo_reply_v6(src: IpAddress, icmp: &mut Vec<u8>) {
    if icmp.len() < 8 {
        return;
    }
    let IpAddress::Ipv6(dst_v6) = src else {
        return;
    };
    if icmp[0] == 128 {
        icmp[0] = 129;
    }
    let ident = u16::from_be_bytes([icmp[4], icmp[5]]);
    let seq_no = u16::from_be_bytes([icmp[6], icmp[7]]);
    let echo_data = icmp[8..].to_vec();
    let repr = Icmpv6Repr::EchoReply {
        ident,
        seq_no,
        data: &echo_data,
    };
    let mut out = vec![0u8; repr.buffer_len()];
    let mut pkt = Icmpv6Packet::new_unchecked(&mut out);
    repr.emit(
        &IpAddress::Ipv6(dst_v6),
        &IpAddress::Ipv6(dst_v6),
        &mut pkt,
        &ChecksumCapabilities::default(),
    );
    *icmp = out;
}

/// Queue an ICMP echo reply locally (self-ping / loopback shortcut).
pub fn queue_echo_reply(src: IpAddress, mut icmp: Vec<u8>) {
    if icmp.is_empty() {
        return;
    }
    match src {
        IpAddress::Ipv4(_) => finalize_icmp_echo_reply_v4(&mut icmp),
        IpAddress::Ipv6(_) => finalize_icmp_echo_reply_v6(src, &mut icmp),
        _ => {}
    }
    let mut q = RX_QUEUE.lock();
    if q.len() >= RX_QUEUE_MAX {
        q.pop_front();
    }
    q.push_back(IcmpRxPacket { src, data: icmp });
    drop(q);
    kernel_hal::net::wake_net_rx_waiters();
}

pub fn pop_for(ipv6: bool, remote: Option<IpAddress>) -> Option<(Vec<u8>, IpAddress)> {
    let mut q = RX_QUEUE.lock();
    let idx = q.iter().position(|pkt| {
        let family_ok = matches!(
            (ipv6, pkt.src),
            (true, IpAddress::Ipv6(_)) | (false, IpAddress::Ipv4(_))
        );
        if !family_ok {
            return false;
        }
        match remote {
            Some(remote_ip) => pkt.src == remote_ip,
            None => true,
        }
    })?;
    q.remove(idx).map(|p| (p.data, p.src))
}

pub fn pending() -> bool {
    !RX_QUEUE.lock().is_empty()
}

/// Returns true if there is at least one queued reply matching the given address family.
/// More precise than `pending()` — avoids spurious wakeups when replies from the
/// opposite family (e.g. ICMPv6 while waiting for ICMPv4) are in the queue.
pub fn pending_for(ipv6: bool) -> bool {
    let q = RX_QUEUE.lock();
    q.iter().any(|pkt| {
        matches!(
            (ipv6, pkt.src),
            (true, IpAddress::Ipv6(_)) | (false, IpAddress::Ipv4(_))
        )
    })
}

/// Build a full IPv4 frame (header + ICMP) for `SOCK_RAW` recv (BusyBox ping as root).
pub fn wrap_icmpv4_raw_frame(src_addr: Ipv4Address, dst_addr: Ipv4Address, icmp: &[u8]) -> Vec<u8> {
    let total = 20 + icmp.len();
    let mut buf = vec![0u8; total];
    let mut pkt = Ipv4Packet::new_unchecked(&mut buf);
    pkt.set_version(4);
    pkt.set_header_len(20);
    pkt.set_total_len(total as u16);
    pkt.set_protocol(IpProtocol::Icmp);
    pkt.set_src_addr(src_addr);
    pkt.set_dst_addr(dst_addr);
    pkt.set_hop_limit(64);
    pkt.payload_mut().copy_from_slice(icmp);
    pkt.fill_checksum();
    buf
}

/// Dequeue an echo reply and wrap it for `SOCK_RAW` + `IPPROTO_ICMP` read(2).
pub fn pop_ipv4_raw_reply(remote: Option<IpAddress>, buf: &mut [u8]) -> Option<(usize, IpAddress)> {
    let (icmp, peer) = pop_for(false, remote)?;
    let IpAddress::Ipv4(peer_v4) = peer else {
        return None;
    };
    let our = crate::net::select_ipv4_for_dst(peer_v4);
    if our.is_unspecified() {
        return None;
    }
    // SOCK_RAW + IPPROTO_ICMP delivers a full IPv4 datagram as received:
    // source = peer that sent the reply, destination = us.
    let frame = wrap_icmpv4_raw_frame(peer_v4, our, &icmp);
    let n = frame.len().min(buf.len());
    buf[..n].copy_from_slice(&frame[..n]);
    Some((n, peer))
}
