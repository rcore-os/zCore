//! Hostname resolver: `/etc/hosts` first, then DNS (A / AAAA) via smoltcp UDP
//! and `/etc/resolv.conf`.

use crate::error::{LxError, LxResult};
use crate::net::{drain_net_poll, local_udp_endpoint_for, UDP_METADATA_BUF};
use alloc::{sync::Arc, vec, vec::Vec};
use core::time::Duration;
use rcore_fs::vfs::INode;
use smoltcp::socket::{UdpPacketMetadata, UdpSocket, UdpSocketBuffer};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address, Ipv6Address};
use zcore_drivers::net::get_sockets;

const DNS_PORT: u16 = 53;
const QTYPE_A: u16 = 1;
const QTYPE_AAAA: u16 = 28;
const QCLASS_IN: u16 = 1;

/// One address returned by [`resolve`].
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DnsResultEntry {
    pub family: u16,
    pub _pad: u16,
    pub addr: [u8; 16],
}

impl DnsResultEntry {
    pub fn from_ip(ip: IpAddress) -> Self {
        let mut addr = [0u8; 16];
        let family = match ip {
            IpAddress::Ipv4(v4) => {
                addr[..4].copy_from_slice(&v4.0);
                2u16
            }
            IpAddress::Ipv6(v6) => {
                addr.copy_from_slice(&v6.0);
                10u16
            }
            IpAddress::Unspecified | _ => 0,
        };
        Self {
            family,
            _pad: 0,
            addr,
        }
    }
}

/// Address family filter for [`resolve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsFamily {
    Unspec,
    V4,
    V6,
}

impl DnsFamily {
    pub fn from_usize(v: usize) -> Self {
        match v {
            2 => DnsFamily::V4,
            10 => DnsFamily::V6,
            _ => DnsFamily::Unspec,
        }
    }
}

/// Resolve `hostname` using `/etc/hosts`, then nameservers from `/etc/resolv.conf`.
pub fn resolve(
    root: &Arc<dyn INode>,
    hostname: &str,
    family: DnsFamily,
) -> LxResult<Vec<IpAddress>> {
    if hostname.is_empty() || hostname.len() > 253 {
        return Err(LxError::EINVAL);
    }

    let hosts = lookup_hosts(root, hostname, family);
    if !hosts.is_empty() {
        return Ok(hosts);
    }

    let servers = read_nameservers(root);
    if servers.is_empty() {
        return Err(LxError::ENOENT);
    }

    let want_v4 = matches!(family, DnsFamily::Unspec | DnsFamily::V4);
    let want_v6 = matches!(family, DnsFamily::Unspec | DnsFamily::V6);
    let mut out = Vec::new();

    for server in servers {
        if want_v4 {
            if let Ok(addrs) = query_at(server, hostname, QTYPE_A) {
                out.extend(addrs);
            }
        }
        if want_v6 {
            if let Ok(addrs) = query_at(server, hostname, QTYPE_AAAA) {
                out.extend(addrs);
            }
        }
        if !out.is_empty() {
            break;
        }
    }

    if out.is_empty() {
        Err(LxError::ENOENT)
    } else {
        Ok(out)
    }
}

fn lookup_hosts(root: &Arc<dyn INode>, hostname: &str, family: DnsFamily) -> Vec<IpAddress> {
    let Ok(inode) = root.lookup("/etc/hosts") else {
        return Vec::new();
    };
    let Ok(meta) = inode.metadata() else {
        return Vec::new();
    };
    let size = meta.size;
    if size == 0 || size > 65536 {
        return Vec::new();
    }
    let mut buf = vec![0u8; size];
    if inode.read_at(0, &mut buf).unwrap_or(0) == 0 {
        return Vec::new();
    }
    let text = core::str::from_utf8(&buf).unwrap_or("");
    let want_v4 = matches!(family, DnsFamily::Unspec | DnsFamily::V4);
    let want_v6 = matches!(family, DnsFamily::Unspec | DnsFamily::V6);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(ip_raw) = parts.next() else { continue };
        let ip = if let Some(v4) = parse_ipv4(ip_raw) {
            if !want_v4 {
                continue;
            }
            IpAddress::Ipv4(v4)
        } else if let Some(v6) = parse_ipv6(ip_raw) {
            if !want_v6 {
                continue;
            }
            IpAddress::Ipv6(v6)
        } else {
            continue;
        };
        for alias in parts {
            if alias.eq_ignore_ascii_case(hostname) {
                if !out.contains(&ip) {
                    out.push(ip);
                }
                break;
            }
        }
    }
    out
}

fn read_nameservers(root: &Arc<dyn INode>) -> Vec<IpAddress> {
    let Ok(inode) = root.lookup("/etc/resolv.conf") else {
        return fallback_nameservers();
    };
    let Ok(meta) = inode.metadata() else {
        return fallback_nameservers();
    };
    let size = meta.size;
    if size == 0 || size > 8192 {
        return fallback_nameservers();
    }
    let mut buf = vec![0u8; size];
    if inode.read_at(0, &mut buf).unwrap_or(0) == 0 {
        return fallback_nameservers();
    }
    let text = core::str::from_utf8(&buf).unwrap_or("");
    let mut servers = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if !line.starts_with("nameserver") {
            continue;
        }
        let mut parts = line.split_whitespace();
        let _ = parts.next();
        let Some(raw) = parts.next() else { continue };
        let scoped = raw.split('%').next().unwrap_or(raw);
        if let Some(v4) = parse_ipv4(scoped) {
            servers.push(IpAddress::Ipv4(v4));
        } else if let Some(v6) = parse_ipv6(scoped) {
            if v6.is_link_local() && !raw.contains('%') {
                continue;
            }
            servers.push(IpAddress::Ipv6(v6));
        }
    }
    if servers.is_empty() {
        return fallback_nameservers();
    }
    for fb in fallback_nameservers() {
        if !servers.contains(&fb) {
            servers.push(fb);
        }
    }
    servers
}

fn fallback_nameservers() -> Vec<IpAddress> {
    vec![
        IpAddress::Ipv4(Ipv4Address::new(8, 8, 8, 8)),
        IpAddress::Ipv4(Ipv4Address::new(1, 1, 1, 1)),
    ]
}

fn parse_ipv4(s: &str) -> Option<Ipv4Address> {
    let mut octets = [0u8; 4];
    let mut parts = s.split('.');
    for o in &mut octets {
        *o = parts.next()?.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(Ipv4Address::from_bytes(&octets))
}

fn parse_ipv6(s: &str) -> Option<Ipv6Address> {
    // Minimal parser: hex groups separated by ':', optional '::' compression.
    // `str::split(':')` yields consecutive empties for leading/trailing `::`
    // (`::1` → ["", "", "1"], `a::` → ["a", "", ""], `::` → ["", "", ""]).
    let s = s.split('%').next().unwrap_or(s);
    if s.is_empty() {
        return None;
    }
    let mut groups = [0u16; 8];
    let mut count = 0usize;
    let mut double_off = None;
    let mut empty_run = 0usize;
    for part in s.split(':') {
        if part.is_empty() {
            empty_run += 1;
            if empty_run == 1 {
                // First empty starts `::` compression (at most one double-colon).
                if double_off.is_some() {
                    return None;
                }
                double_off = Some(count);
            } else if empty_run == 2 {
                // Second consecutive empty: leading or trailing `::` artifact.
            } else if empty_run == 3 && count == 0 && double_off == Some(0) {
                // Bare `::` yields three empties.
            } else {
                return None;
            }
            continue;
        }
        if empty_run == 3 {
            return None; // e.g. `:::1`
        }
        // Lone leading ':' (`:1`): one empty then a group without a second empty.
        if empty_run == 1 && count == 0 && double_off == Some(0) {
            return None;
        }
        empty_run = 0;
        let value = u16::from_str_radix(part, 16).ok()?;
        if count >= 8 {
            return None;
        }
        groups[count] = value;
        count += 1;
    }
    // Lone trailing ':' (`1:`) is a single empty at end — not valid `::`.
    if empty_run == 1 && count > 0 {
        return None;
    }
    let tail = if let Some(at) = double_off {
        let head = at;
        let rest = count - at;
        let zeros = 8usize.checked_sub(rest + head)?;
        if zeros == 0 {
            return None;
        }
        let mut out = [0u16; 8];
        out[..head].copy_from_slice(&groups[..head]);
        let tail_start = 8 - rest;
        out[tail_start..].copy_from_slice(&groups[at..count]);
        out
    } else if count == 8 {
        groups
    } else {
        return None;
    };
    let mut bytes = [0u8; 16];
    for (i, g) in tail.iter().enumerate() {
        bytes[i * 2] = (g >> 8) as u8;
        bytes[i * 2 + 1] = (g & 0xff) as u8;
    }
    Some(Ipv6Address::from_bytes(&bytes))
}

fn query_at(server: IpAddress, name: &str, qtype: u16) -> LxResult<Vec<IpAddress>> {
    let id = (super::rand() & 0xffff) as u16;
    let query = build_query(name, id, qtype)?;
    let (local, remote) = match server {
        IpAddress::Ipv4(v4) => (
            local_udp_endpoint_for(IpAddress::Ipv4(v4)),
            IpEndpoint::new(IpAddress::Ipv4(v4), DNS_PORT),
        ),
        IpAddress::Ipv6(v6) => (
            local_udp_endpoint_for(IpAddress::Ipv6(v6)),
            IpEndpoint::new(IpAddress::Ipv6(v6), DNS_PORT),
        ),
        IpAddress::Unspecified | _ => return Err(LxError::EINVAL),
    };
    let reply = udp_exchange(local, remote, &query, id)?;
    parse_addresses(&reply, qtype)
}

fn build_query(name: &str, id: u16, qtype: u16) -> LxResult<Vec<u8>> {
    let mut qname = Vec::new();
    encode_qname(name, &mut qname)?;
    let mut out = Vec::with_capacity(12 + qname.len() + 4);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&[0x01, 0x00]); // RD=1
    out.extend_from_slice(&[0x00, 0x01]); // QDCOUNT=1
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // AN/NS/AR
    out.extend_from_slice(&qname);
    out.extend_from_slice(&qtype.to_be_bytes());
    out.extend_from_slice(&QCLASS_IN.to_be_bytes());
    Ok(out)
}

fn encode_qname(name: &str, out: &mut Vec<u8>) -> LxResult {
    if name.is_empty() {
        out.push(0);
        return Ok(());
    }
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(LxError::EINVAL);
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    Ok(())
}

fn spin_ms(ms: u64) {
    let deadline = kernel_hal::timer::timer_now() + Duration::from_millis(ms);
    while kernel_hal::timer::timer_now() < deadline {
        core::hint::spin_loop();
    }
}

fn udp_exchange(
    local: IpEndpoint,
    remote: IpEndpoint,
    query: &[u8],
    expect_id: u16,
) -> LxResult<Vec<u8>> {
    let rx_buffer = UdpSocketBuffer::new(
        vec![UdpPacketMetadata::EMPTY; UDP_METADATA_BUF],
        vec![0u8; 512],
    );
    let tx_buffer = UdpSocketBuffer::new(
        vec![UdpPacketMetadata::EMPTY; UDP_METADATA_BUF],
        vec![0u8; 512],
    );
    let socket = UdpSocket::new(rx_buffer, tx_buffer);
    let sockets = get_sockets();
    let mut set = sockets.lock();
    if super::smoltcp_socket_count(&set) >= super::MAX_SMOLTCIP_SOCKETS {
        return Err(LxError::ENOMEM);
    }
    let handle = set.add(socket);
    drop(set);
    {
        let mut set = sockets.lock();
        let mut sock = set.get::<UdpSocket>(handle);
        sock.bind(local).map_err(|_| LxError::EINVAL)?;
        sock.send_slice(query, remote).map_err(|_| LxError::EIO)?;
    }

    let mut buf = [0u8; 512];
    for round in 0..32 {
        drain_net_poll(8);
        if round == 0 {
            kernel_hal::deferred_job::drain_deferred_jobs();
        }
        let mut set = sockets.lock();
        let mut sock = set.get::<UdpSocket>(handle);
        if sock.can_recv() {
            if let Ok((n, _)) = sock.recv_slice(&mut buf) {
                if n >= 2 {
                    let id = u16::from_be_bytes([buf[0], buf[1]]);
                    if id == expect_id {
                        drop(sock);
                        drop(set);
                        sockets.lock().remove(handle);
                        return Ok(buf[..n].to_vec());
                    }
                }
            }
        }
        drop(sock);
        drop(set);
        spin_ms(50);
    }
    sockets.lock().remove(handle);
    Err(LxError::ETIMEDOUT)
}

fn skip_name(data: &[u8], mut off: usize) -> Option<usize> {
    let mut jumps = 0;
    loop {
        if off >= data.len() {
            return None;
        }
        let len = data[off];
        if len == 0 {
            return Some(off + 1);
        }
        if len & 0xc0 == 0xc0 {
            if off + 1 >= data.len() {
                return None;
            }
            return Some(off + 2);
        }
        off += 1 + len as usize;
        jumps += 1;
        if jumps > 128 {
            return None;
        }
    }
}

fn parse_addresses(data: &[u8], qtype: u16) -> LxResult<Vec<IpAddress>> {
    if data.len() < 12 {
        return Err(LxError::EINVAL);
    }
    let rcode = data[3] & 0x0f;
    if rcode != 0 {
        return Err(LxError::EIO);
    }
    let qd = u16::from_be_bytes([data[4], data[5]]) as usize;
    let an = u16::from_be_bytes([data[6], data[7]]) as usize;
    let mut off = 12usize;
    for _ in 0..qd {
        off = skip_name(data, off).ok_or(LxError::EINVAL)?;
        off += 4;
        if off > data.len() {
            return Err(LxError::EINVAL);
        }
    }
    let mut addrs = Vec::new();
    for _ in 0..an {
        off = skip_name(data, off).ok_or(LxError::EINVAL)?;
        if off + 10 > data.len() {
            break;
        }
        let rtype = u16::from_be_bytes([data[off], data[off + 1]]);
        let rdlen = u16::from_be_bytes([data[off + 8], data[off + 9]]) as usize;
        off += 10;
        if off + rdlen > data.len() {
            break;
        }
        let rdata = &data[off..off + rdlen];
        if rtype == qtype {
            match qtype {
                QTYPE_A if rdlen == 4 => {
                    addrs.push(IpAddress::Ipv4(Ipv4Address::from_bytes(rdata)));
                }
                QTYPE_AAAA if rdlen == 16 => {
                    addrs.push(IpAddress::Ipv6(Ipv6Address::from_bytes(rdata)));
                }
                _ => {}
            }
        }
        off += rdlen;
    }
    if addrs.is_empty() {
        Err(LxError::ENOENT)
    } else {
        Ok(addrs)
    }
}
