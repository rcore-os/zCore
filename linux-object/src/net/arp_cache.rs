//! Software IPv4 → Ethernet cache fed from every dispatched RX frame.
//! DHCP uses `NetScheme::send` directly; ping uses this to reach the gateway
//! without waiting on smoltcp egress/neighbor state.

use alloc::collections::BTreeMap;
use lock::Mutex;
use smoltcp::wire::{ArpOperation, ArpPacket, ArpRepr, EthernetAddress, Ipv4Address};

use lazy_static::lazy_static;

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

/// Cap software ARP cache — every RX frame can learn a new entry.
const CACHE_MAX: usize = 512;

/// How long a learned entry stays valid (ms). After this it is treated as stale
/// and re-resolved, so a peer that changed MAC (reboot/re-home) — or a spoofed
/// entry — does not stick forever.
const REACHABLE_MS: u64 = 60_000;

fn now_ms() -> u64 {
    kernel_hal::timer::timer_now().as_millis() as u64
}

lazy_static! {
    /// value = (MAC, learn timestamp in ms) for TTL expiry and LRU eviction.
    static ref CACHE: Mutex<BTreeMap<Ipv4Address, (EthernetAddress, u64)>> =
        Mutex::new(BTreeMap::new());
}

fn insert_bounded(
    map: &mut BTreeMap<Ipv4Address, (EthernetAddress, u64)>,
    ip: Ipv4Address,
    mac: EthernetAddress,
) {
    if map.len() >= CACHE_MAX && !map.contains_key(&ip) {
        // Evict the OLDEST entry (by learn time), not the numerically smallest
        // IP: the latter let an attacker deterministically flush a chosen entry
        // (e.g. the gateway) by flooding spoofed frames with higher source IPs.
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
    if !src_mac.is_unicast() {
        return;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    match ethertype {
        0x0800 => {
            if frame.len() < 34 {
                return;
            }
            let ihl = ((frame[14] & 0x0f) as usize) * 4;
            if frame.len() < 14 + ihl + 4 {
                return;
            }
            let src = Ipv4Address::from_bytes(&frame[26..30]);
            // QEMU slirp DHCP can carry server IP (10.0.2.2) with our own L2 source — skip.
            if src.is_unicast() && !src.is_unspecified() && !is_local_mac(src_mac) {
                insert_bounded(&mut CACHE.lock(), src, src_mac);
            }
        }
        0x0806 => {
            if frame.len() < 42 {
                return;
            }
            let arp = ArpPacket::new_unchecked(&frame[14..]);
            if let Ok(ArpRepr::EthernetIpv4 {
                operation,
                source_protocol_addr,
                source_hardware_addr,
                ..
            }) = ArpRepr::parse(&arp)
            {
                if matches!(operation, ArpOperation::Request | ArpOperation::Reply)
                    && source_protocol_addr.is_unicast()
                {
                    insert_bounded(
                        &mut CACHE.lock(),
                        source_protocol_addr,
                        source_hardware_addr,
                    );
                }
            }
        }
        _ => {}
    }
}

pub fn lookup(dst: Ipv4Address) -> Option<EthernetAddress> {
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

pub fn remove(dst: Ipv4Address) {
    CACHE.lock().remove(&dst);
}

pub fn clear() {
    CACHE.lock().clear();
}

pub fn insert(dst: Ipv4Address, mac: EthernetAddress) {
    if dst.is_unicast() {
        insert_bounded(&mut CACHE.lock(), dst, mac);
    }
}

pub fn get_entries() -> alloc::vec::Vec<(Ipv4Address, EthernetAddress)> {
    CACHE
        .lock()
        .iter()
        .map(|(&ip, &(mac, _))| (ip, mac))
        .collect()
}
