//! IPv6 Router Advertisement (ICMPv6 type 134) processing.
//!
//! smoltcp never acts on Router Advertisements, and DHCPv6 (IA_NA) only conveys
//! a /128 host address — neither installs a default route nor learns the on-link
//! prefix. Without that, every off-link IPv6 destination (and every reply to an
//! off-link peer) has no next-hop, so global IPv6 is effectively unroutable in
//! both directions even though the address is configured.
//!
//! This module is fed every RX frame from `push_packet` (the same path as
//! `ndp_cache` / `icmp_rx`). For a valid RA it:
//!   * installs a default IPv6 route (`::/0`) via the router's link-local source,
//!     which gives every IPv6 destination a reachable, NDP-resolvable next-hop;
//!   * performs SLAAC (RFC 4862) for an autonomous on-link /64 prefix, forming
//!     `prefix || EUI-64(MAC)` and assigning it as a /64 so the prefix is on-link.
//!
//! The RA is parsed by hand rather than via `NdiscRepr::parse`, because that
//! parser rejects the whole advertisement when it carries an option it does not
//! model (RDNSS, Route Information, …) — options real routers routinely send.

use kernel_hal::net::get_net_device;
use lazy_static::lazy_static;
use lock::Mutex;
use log::*;
use smoltcp::wire::{
    Icmpv6Packet, IpAddress, IpCidr, IpProtocol, Ipv6Address, Ipv6Cidr, Ipv6Packet,
};

const ICMPV6_ROUTER_ADVERT: u8 = 134;
/// Prefix Information option (RFC 4861 §4.6.2).
const OPT_PREFIX_INFORMATION: u8 = 3;
const PIO_FLAG_ONLINK: u8 = 0x80;
const PIO_FLAG_AUTONOMOUS: u8 = 0x40;

/// Bound how many default routes / SLAAC addresses RA processing may install, so
/// an on-link attacker flooding RAs with varied source link-locals or prefixes
/// cannot grow the routing table / interface address list without limit.
const MAX_RA_ROUTES: usize = 8;
const MAX_RA_SLAAC: usize = 8;

/// Last applied configuration, to keep periodic re-advertisements idempotent.
struct RaState {
    gateway: Option<Ipv6Address>,
    slaac: Option<Ipv6Address>,
    /// Count of routes/addresses this module has installed (bounded above).
    installed_routes: usize,
    installed_slaac: usize,
}

lazy_static! {
    static ref STATE: Mutex<RaState> = Mutex::new(RaState {
        gateway: None,
        slaac: None,
        installed_routes: 0,
        installed_slaac: 0,
    });
}

/// (prefix bytes, prefix length, PIO flags, valid lifetime seconds).
type PrefixInfo = ([u8; 16], u8, u8, u32);

/// Inspect a received Ethernet frame and act on an IPv6 Router Advertisement.
pub fn process_from_frame(frame: &[u8]) {
    if frame.len() < 14 {
        return;
    }
    // Ethernet header, optionally one 802.1Q VLAN tag, then IPv6.
    let mut l2 = 14usize;
    let mut et = u16::from_be_bytes([frame[12], frame[13]]);
    if et == 0x8100 {
        if frame.len() < 18 {
            return;
        }
        l2 = 18;
        et = u16::from_be_bytes([frame[16], frame[17]]);
    }
    if et != 0x86dd {
        return;
    }

    let ipv6 = match Ipv6Packet::new_checked(&frame[l2..]) {
        Ok(p) => p,
        Err(_) => return,
    };
    if ipv6.next_header() != IpProtocol::Icmpv6 {
        return;
    }
    // RFC 4861 §6.1.2: a received RA MUST have IPv6 Hop Limit 255. An off-link
    // attacker cannot forge this (any intermediate router decrements it), so
    // this rejects off-link rogue-RA spoofing.
    if ipv6.hop_limit() != 255 {
        return;
    }
    let src = ipv6.src_addr();
    // RFC 4861 §6.1.2: a valid RA is always sourced from a link-local address.
    if !src.is_link_local() {
        return;
    }
    let dst = ipv6.dst_addr();

    let payload = ipv6.payload();
    if payload.len() < 16 || payload[0] != ICMPV6_ROUTER_ADVERT {
        return;
    }
    // Validate the ICMPv6 checksum before trusting any field.
    match Icmpv6Packet::new_checked(payload) {
        Ok(icmp) => {
            if !icmp.verify_checksum(&IpAddress::Ipv6(src), &IpAddress::Ipv6(dst)) {
                return;
            }
        }
        Err(_) => return,
    }

    // RA header: [4]=cur hop limit, [5]=flags, [6..8]=router lifetime (s),
    // [8..12]=reachable time, [12..16]=retrans timer, [16..]=options.
    let router_lifetime = u16::from_be_bytes([payload[6], payload[7]]);

    let mut prefix: Option<PrefixInfo> = None;
    let mut off = 16usize;
    while off + 2 <= payload.len() {
        let opt_type = payload[off];
        let units = payload[off + 1] as usize;
        if units == 0 {
            break; // malformed: every option length is >= 1 unit (8 bytes)
        }
        let opt_len = units * 8;
        if off + opt_len > payload.len() {
            break;
        }
        if opt_type == OPT_PREFIX_INFORMATION && opt_len >= 32 {
            let plen = payload[off + 2];
            let flags = payload[off + 3];
            let valid = u32::from_be_bytes([
                payload[off + 4],
                payload[off + 5],
                payload[off + 6],
                payload[off + 7],
            ]);
            let mut pfx = [0u8; 16];
            pfx.copy_from_slice(&payload[off + 16..off + 32]);
            prefix = Some((pfx, plen, flags, valid));
            break; // single-prefix model is enough for the common case
        }
        off += opt_len;
    }

    apply(src, router_lifetime, prefix);
}

fn apply(router_ll: Ipv6Address, router_lifetime: u16, prefix: Option<PrefixInfo>) {
    // Apply to the primary Ethernet interface. The RX dispatch path carries no
    // ingress-interface information (frames are processed globally, as with
    // `ndp_cache`/`icmp_rx`), so on a multi-NIC host the RA is attributed to the
    // first Ethernet device — correct for the common single-NIC case.
    let iface = match get_net_device()
        .into_iter()
        .find(|d| d.get_ifname() != "loopback")
    {
        Some(d) => d,
        None => return,
    };

    // --- Default IPv6 route via the router's link-local source ---
    // A lifetime of 0 means "not a default router"; if this is our current
    // gateway, delete the default route and clear gateway state.
    let default_cidr = IpCidr::Ipv6(Ipv6Cidr::new(Ipv6Address::UNSPECIFIED, 0));
    if router_lifetime == 0 {
        let mut st = STATE.lock();
        if st.gateway == Some(router_ll) {
            let _ = iface.del_route(default_cidr, Some(IpAddress::Ipv6(router_ll)));
            st.gateway = None;
            st.installed_routes = st.installed_routes.saturating_sub(1);
            info!(
                "[ra] removed default IPv6 route via {} on {}",
                router_ll,
                iface.get_ifname()
            );
        }
    } else {
        let mut st = STATE.lock();
        if st.gateway != Some(router_ll) && st.installed_routes < MAX_RA_ROUTES {
            if iface
                .add_route(default_cidr, Some(IpAddress::Ipv6(router_ll)))
                .is_ok()
            {
                st.gateway = Some(router_ll);
                st.installed_routes += 1;
                info!(
                    "[ra] default IPv6 route via {} on {}",
                    router_ll,
                    iface.get_ifname()
                );
            }
        }
    }

    // --- SLAAC for an autonomous on-link /64 prefix ---
    if let Some((pfx, plen, flags, valid)) = prefix {
        let usable = (flags & PIO_FLAG_AUTONOMOUS) != 0
            && (flags & PIO_FLAG_ONLINK) != 0
            && valid > 0
            && plen == 64;
        if usable {
            // Interface identifier = EUI-64 of the MAC (the low 64 bits of the
            // RFC 4862 link-local address derived from the same MAC).
            let ll = crate::net::ipv6_link_local_from_mac(&iface.get_mac());
            let iid = ll.as_bytes();
            let mut addr = [0u8; 16];
            addr[..8].copy_from_slice(&pfx[..8]);
            addr[8..].copy_from_slice(&iid[8..]);
            let global = Ipv6Address::from_bytes(&addr);
            let cidr = IpCidr::Ipv6(Ipv6Cidr::new(global, 64));

            let mut st = STATE.lock();
            let already = iface.get_ip_address().contains(&cidr);
            if !already && st.installed_slaac < MAX_RA_SLAAC && iface.add_ip_address(cidr).is_ok() {
                st.slaac = Some(global);
                st.installed_slaac += 1;
                info!("[ra] SLAAC {}/64 on {}", global, iface.get_ifname());
            }
        }
    }
}
