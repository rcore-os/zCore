//! Software IPv6 → Ethernet cache fed from every dispatched RX frame.

use alloc::collections::BTreeMap;
use lazy_static::lazy_static;
use lock::Mutex;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    EthernetAddress, Icmpv6Packet, Icmpv6Repr, IpAddress, Ipv6Address, Ipv6Packet, NdiscRepr,
};

lazy_static! {
    static ref LOCAL_MACS: Mutex<alloc::vec::Vec<EthernetAddress>> =
        Mutex::new(alloc::vec::Vec::new());
}

/// Refresh cached NIC MACs (call after probe / NewAddr).
pub fn refresh_local_macs(macs: alloc::vec::Vec<EthernetAddress>) {
    *LOCAL_MACS.lock() = macs;
}

fn is_local_mac(mac: EthernetAddress) -> bool {
    LOCAL_MACS.lock().contains(&mac)
}

const CACHE_MAX: usize = 512;

/// How long a learned entry stays valid (ms) before it is re-resolved.
const REACHABLE_MS: u64 = 60_000;

fn now_ms() -> u64 {
    kernel_hal::timer::timer_now().as_millis() as u64
}

lazy_static! {
    /// value = (MAC, learn timestamp in ms) for TTL expiry and LRU eviction.
    static ref CACHE: Mutex<BTreeMap<Ipv6Address, (EthernetAddress, u64)>> =
        Mutex::new(BTreeMap::new());
}

fn insert_bounded(
    map: &mut BTreeMap<Ipv6Address, (EthernetAddress, u64)>,
    ip: Ipv6Address,
    mac: EthernetAddress,
) {
    if map.len() >= CACHE_MAX && !map.contains_key(&ip) {
        // Evict the OLDEST entry (by learn time), not the numerically smallest
        // IP, so an attacker cannot deterministically flush a chosen entry.
        if let Some(old) = map.iter().min_by_key(|(_, (_, ts))| *ts).map(|(&ip, _)| ip) {
            map.remove(&old);
        }
    }
    map.insert(ip, (mac, now_ms()));
}

/// Learn mappings from a complete Ethernet frame (called from `push_packet`).
pub fn learn_from_frame(frame: &[u8]) {
    if frame.len() < 14 {
        return;
    }
    let src_mac = EthernetAddress::from_bytes(&frame[6..12]);
    if !src_mac.is_unicast() || is_local_mac(src_mac) {
        return;
    }
    // Ethernet header, optionally one 802.1Q VLAN tag, then IPv6.
    let mut l2 = 14usize;
    let mut ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype == 0x8100 {
        if frame.len() < 18 {
            return;
        }
        l2 = 18;
        ethertype = u16::from_be_bytes([frame[16], frame[17]]);
    }
    if ethertype != 0x86dd {
        return;
    }
    let ipv6 = match Ipv6Packet::new_checked(&frame[l2..]) {
        Ok(pkt) => pkt,
        Err(_) => return,
    };
    let src_ip = ipv6.src_addr();
    if src_ip.is_unicast() && !src_ip.is_unspecified() {
        insert_bounded(&mut CACHE.lock(), src_ip, src_mac);
    }

    if ipv6.next_header() != smoltcp::wire::IpProtocol::Icmpv6 {
        return;
    }
    let icmp = match Icmpv6Packet::new_checked(ipv6.payload()) {
        Ok(pkt) => pkt,
        Err(_) => return,
    };
    let repr = match Icmpv6Repr::parse(
        &IpAddress::Ipv6(ipv6.src_addr()),
        &IpAddress::Ipv6(ipv6.dst_addr()),
        &icmp,
        &ChecksumCapabilities::default(),
    ) {
        Ok(r) => r,
        Err(_) => return,
    };

    match repr {
        // Neighbor Advertisement: `target_addr` is the address the sender OWNS
        // and `lladdr` is its Target Link-Layer Address, so `target_addr ->
        // lladdr` is the correct mapping.
        Icmpv6Repr::Ndisc(NdiscRepr::NeighborAdvert {
            target_addr,
            lladdr,
            ..
        }) if target_addr.is_unicast() && !target_addr.is_unspecified() => {
            insert_bounded(&mut CACHE.lock(), target_addr, lladdr.unwrap_or(src_mac));
        }
        // Neighbor Solicitation: `target_addr` is the address being QUERIED (it
        // is NOT owned by the sender) and `lladdr` is the sender's Source
        // Link-Layer Address. Learning `target_addr -> lladdr` here would map the
        // queried IP to the querier's MAC -- a bogus/poisoned entry that redirects
        // traffic for `target_addr` to whoever solicited it, and corrupts the
        // cache even under benign NS traffic. The correct `src_ip -> src_mac`
        // mapping is already learned from the IPv6 header above.
        _ => {}
    }
}

pub fn lookup(dst: Ipv6Address) -> Option<EthernetAddress> {
    let mut cache = CACHE.lock();
    let (mac, ts) = *cache.get(&dst)?;
    // Expire stale entries so a changed/spoofed MAC is re-resolved.
    if now_ms().saturating_sub(ts) > REACHABLE_MS {
        cache.remove(&dst);
        return None;
    }
    if is_local_mac(mac) {
        return None;
    }
    Some(mac)
}

pub fn clear() {
    CACHE.lock().clear();
}

pub fn get_entries() -> alloc::vec::Vec<(Ipv6Address, EthernetAddress)> {
    CACHE
        .lock()
        .iter()
        .map(|(&ip, &(mac, _))| (ip, mac))
        .collect()
}
