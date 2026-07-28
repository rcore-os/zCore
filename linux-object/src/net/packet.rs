use crate::{
    error::{LxError, LxResult},
    fs::{FileLike, OpenFlags, PollEvents, PollStatus},
    net::*,
};
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use async_trait::async_trait;

// use kernel_hal::user::UserInOutPtr;
use kernel_hal::{net::get_net_device, thread};
use lazy_static::lazy_static;
use lock::Mutex;
use smoltcp::wire::{EthernetAddress, EthernetFrame};
use zcore_drivers::scheme::NetScheme;
use zircon_object::object::*;

/// Per-socket RX queue depth (enough for DHCP bursts).
const PACKET_QUEUE_MAX: usize = 16;
/// Max concurrent AF_PACKET socket groups (each may queue PACKET_QUEUE_MAX frames).
const MAX_PACKET_SOCKETS: usize = 12;
/// Max Ethernet frame size we build on the stack for SOCK_DGRAM TX.
const ETH_FRAME_MAX: usize = 1518;

/// Shared RX frame storage — one heap copy per frame, refcounted across sockets.
type PacketFrame = Arc<[u8]>;

lazy_static! {
    static ref PACKET_SOCKETS: Mutex<Vec<Weak<PacketSocketInner>>> = Mutex::new(Vec::new());
}

fn purge_dead_sockets(sockets: &mut Vec<Weak<PacketSocketInner>>) {
    sockets.retain(|w| w.strong_count() > 0);
}

fn unregister_inner(inner: &Arc<PacketSocketInner>) {
    let inner_ptr = Arc::as_ptr(inner);
    PACKET_SOCKETS.lock().retain(|weak| {
        weak.upgrade()
            .map(|arc| !core::ptr::eq(Arc::as_ptr(&arc), inner_ptr))
            .unwrap_or(false)
    });
}

fn register_fd(inner: &Arc<PacketSocketInner>, state: &Arc<PacketSocketState>) {
    let mut fds = inner.fds.lock();
    fds.push(Arc::downgrade(state));
    fds.retain(|w| w.strong_count() > 0);
}

fn wake_readers(inner: &PacketSocketInner) {
    let mut fds = inner.fds.lock();
    fds.retain(|w| {
        if let Some(s) = w.upgrade() {
            s.base.signal_set(Signal::READABLE);
            true
        } else {
            false
        }
    });
}

const MAX_FRAME_COPY: usize = 1518;

/// Bytes of L2 header (14, or 18 with 802.1Q) and EtherType.
fn eth_l2_header_len(frame: &[u8]) -> Option<(usize, u16)> {
    if frame.len() < 14 {
        return None;
    }
    let et = u16::from_be_bytes([frame[12], frame[13]]);
    if et == 0x8100 {
        if frame.len() < 18 {
            return None;
        }
        Some((18, u16::from_be_bytes([frame[16], frame[17]])))
    } else {
        Some((14, et))
    }
}

/// SOCK_DGRAM AF_PACKET delivers a bare IPv4 datagram. udhcpc's get_raw_packet()
/// requires `read_len - iphdr_len == udp.len` exactly — IP padding after the UDP
/// segment must not be included (Linux trims to the UDP length for this path).
fn ipv4_datagram_from_eth_frame(frame: &[u8]) -> Option<&[u8]> {
    let (l2, et) = eth_l2_header_len(frame)?;
    if et != 0x0800 || frame.len() < l2 + 20 {
        return None;
    }
    let ihl = ((frame[l2] & 0x0f) as usize) * 4;
    if ihl < 20 || frame.len() < l2 + ihl {
        return None;
    }
    let ip_tot = u16::from_be_bytes([frame[l2 + 2], frame[l2 + 3]]) as usize;
    if ip_tot < ihl || l2 + ip_tot > frame.len() {
        return None;
    }
    let mut end = l2 + ip_tot;
    if frame[l2 + 9] == 17 && frame.len() >= l2 + ihl + 8 {
        let udp_len = u16::from_be_bytes([frame[l2 + ihl + 4], frame[l2 + ihl + 5]]) as usize;
        if (8..=ip_tot.saturating_sub(ihl)).contains(&udp_len) {
            end = l2 + ihl + udp_len;
        }
    }
    Some(&frame[l2..end])
}

fn frame_ethertype(packet: &[u8]) -> Option<u16> {
    eth_l2_header_len(packet).map(|(_, et)| et)
}

fn frame_arc(packet: &[u8]) -> Option<PacketFrame> {
    let n = packet.len().min(MAX_FRAME_COPY);
    if n == 0 {
        return None;
    }
    let mut buf = Vec::with_capacity(n);
    buf.extend_from_slice(&packet[..n]);
    Some(Arc::from(buf.into_boxed_slice()))
}

/// Dispatches a received packet to all registered AF_PACKET sockets.
///
/// Do not call `intr_on()`/`intr_off()` here: `lock::Mutex` already uses
/// `push_off`/`pop_off` on a per-CPU `RefCell`. Re-enabling IRQs while a
/// mutex guard is held breaks that nesting and panics with "RefCell already borrowed".
pub fn push_packet(packet: &[u8]) {
    crate::net::arp_cache::learn_from_frame(packet);
    crate::net::ndp_cache::learn_from_frame(packet);
    crate::net::icmp_rx::deliver_from_frame(packet);
    crate::net::ra::process_from_frame(packet);

    let mut sockets = PACKET_SOCKETS.lock();
    if !sockets.iter().any(|w| w.strong_count() > 0) {
        purge_dead_sockets(&mut sockets);
        return;
    }
    let mut to_remove = Vec::new();
    let mut shared: Option<PacketFrame> = None;
    let ethertype = frame_ethertype(packet);

    for (i, weak) in sockets.iter().enumerate() {
        if let Some(inner) = weak.upgrade() {
            let protocol = *inner.protocol.lock();
            if let Some(et) = ethertype {
                if protocol != 0 && protocol != 0x0003 && protocol != et {
                    continue;
                }

                let mut queue = inner.packet_queue.lock();
                if queue.len() >= PACKET_QUEUE_MAX {
                    queue.pop_front();
                }
                if shared.is_none() {
                    shared = frame_arc(packet);
                }
                if let Some(arc) = &shared {
                    queue.push_back(Arc::clone(arc));
                }
                drop(queue);
                wake_readers(&inner);
            }
        } else {
            to_remove.push(i);
        }
    }

    for i in to_remove.into_iter().rev() {
        sockets.swap_remove(i);
    }
    purge_dead_sockets(&mut sockets);
}

pub struct PacketSocketState {
    base: KObjectBase,
    inner: Arc<PacketSocketInner>,
}

#[derive(Debug)]
struct PacketSocketInner {
    flags: Mutex<OpenFlags>,
    ifindex: Mutex<u32>,
    socket_type: SocketType,
    protocol: Mutex<u16>,
    packet_queue: Mutex<VecDeque<PacketFrame>>,
    /// All file descriptors sharing this queue (original + dup).
    fds: Mutex<Vec<Weak<PacketSocketState>>>,
}

impl PacketSocketState {
    pub fn new(socket_type: SocketType, protocol: u16) -> LxResult<Arc<Self>> {
        let mut registry = PACKET_SOCKETS.lock();
        purge_dead_sockets(&mut registry);
        if registry.iter().filter(|w| w.strong_count() > 0).count() >= MAX_PACKET_SOCKETS {
            return Err(LxError::ENOMEM);
        }

        let inner = Arc::new(PacketSocketInner {
            flags: Mutex::new(OpenFlags::RDWR),
            ifindex: Mutex::new(0),
            socket_type,
            protocol: Mutex::new(protocol),
            packet_queue: Mutex::new(VecDeque::new()),
            fds: Mutex::new(Vec::new()),
        });
        let state = Arc::new(Self {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            inner: inner.clone(),
        });
        register_fd(&inner, &state);
        registry.push(Arc::downgrade(&inner));
        Ok(state)
    }
}

impl Drop for PacketSocketState {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            unregister_inner(&self.inner);
        }
    }
}

#[async_trait]
impl Socket for PacketSocketState {
    async fn read(&self, data: &mut [u8]) -> (SysResult, Endpoint) {
        let mut endpoint =
            Endpoint::LinkLevel(LinkLevelEndpoint::new(*self.inner.ifindex.lock() as usize));
        let non_block = self.inner.flags.lock().contains(OpenFlags::NON_BLOCK);

        loop {
            // Always drain deferred jobs and poll the NIC on every iteration.
            // On real hardware the packet queue may contain network noise (ARP probes,
            // mDNS, etc.) that keeps it non-empty, which would prevent net.poll()
            // from ever running and leave DHCPACK stuck in the hardware RX ring.
            kernel_hal::deferred_job::drain_deferred_jobs();

            let ifindex = *self.inner.ifindex.lock();
            {
                let poll_net = |net: &(dyn NetScheme + Send + Sync)| {
                    let _ = net.poll();
                    let _ = net.poll();
                };
                if ifindex > 0 {
                    if let Ok(net) = crate::net::iface_by_linux_ifindex(ifindex) {
                        poll_net(net.as_ref());
                    }
                } else {
                    for net in get_net_device().iter() {
                        if net.get_ifname() != "loopback" {
                            poll_net(net.as_ref());
                        }
                    }
                }
            }

            let pkt = self.inner.packet_queue.lock().pop_front();
            if let Some(internal_buf) = pkt {
                let n = internal_buf.len();
                if let Some((_, et)) = eth_l2_header_len(&internal_buf[..n]) {
                    if let Endpoint::LinkLevel(ref mut ll) = endpoint {
                        ll.addr[..6].copy_from_slice(&internal_buf[6..12]);
                        ll.halen = 6;
                        ll.protocol = et;
                    }
                }
                let payload = if self.inner.socket_type == SocketType::SOCK_DGRAM {
                    ipv4_datagram_from_eth_frame(&internal_buf[..n]).unwrap_or_else(|| {
                        if n > 14 {
                            &internal_buf[14..n]
                        } else {
                            &internal_buf[..0]
                        }
                    })
                } else {
                    &internal_buf[..n]
                };
                let actual_len = payload.len();
                let copy_len = actual_len.min(data.len());
                data[..copy_len].copy_from_slice(&payload[..copy_len]);

                if self.inner.packet_queue.lock().is_empty() {
                    self.base.signal_clear(Signal::READABLE);
                }

                // Return the number of bytes actually copied, NOT the full frame
                // length. The syscall layer slices the caller's buffer by the
                // returned count (`&data[..n]`); returning `actual_len` when the
                // frame is larger than the user buffer slices out of bounds and
                // panics the kernel (every other socket type returns copy_len).
                return (Ok(copy_len), endpoint);
            }

            if non_block {
                return (Err(LxError::EAGAIN), endpoint);
            }

            // Drain deferred jobs (IRQ -> iface.poll -> push_packet) and then sleep a short
            // interval. On real hardware the NIC IRQ enqueues a deferred_job; draining here
            // ensures we don't miss a packet that arrived just before we slept.
            kernel_hal::deferred_job::drain_deferred_jobs();
            thread::sleep_until(
                kernel_hal::timer::timer_now() + core::time::Duration::from_millis(5),
            )
            .await;
        }
    }
    fn write(&self, data: &[u8], sendto_endpoint: Option<Endpoint>) -> SysResult {
        let ifindex = *self.inner.ifindex.lock();
        let dev = if ifindex > 0 {
            crate::net::iface_by_linux_ifindex(ifindex)?
        } else {
            crate::net::netdev_for_ipv4(smoltcp::wire::Ipv4Address::UNSPECIFIED).or_else(|_| {
                get_net_device()
                    .into_iter()
                    .find(|n| n.get_ifname() != "loopback")
                    .ok_or(LxError::ENODEV)
            })?
        };

        if self.inner.socket_type == SocketType::SOCK_DGRAM {
            if let Some(Endpoint::LinkLevel(ll)) = sendto_endpoint {
                if data.len() + 14 > ETH_FRAME_MAX {
                    return Err(LxError::EINVAL);
                }
                let mut buf = [0u8; ETH_FRAME_MAX];
                let frame_len = data.len() + 14;
                let mut frame = EthernetFrame::new_unchecked(&mut buf[..frame_len]);
                frame.set_dst_addr(EthernetAddress::from_bytes(&ll.addr[..6]));
                frame.set_src_addr(dev.get_mac());
                let protocol_raw = if ll.protocol != 0 {
                    ll.protocol
                } else {
                    *self.inner.protocol.lock()
                };
                let protocol = protocol_raw;
                frame.set_ethertype(protocol.into());
                frame.payload_mut().copy_from_slice(data);
                for _ in 0..16 {
                    if dev.send(&buf[..frame_len]).is_ok() {
                        kernel_hal::deferred_job::drain_deferred_jobs();
                        return Ok(data.len());
                    }
                    kernel_hal::deferred_job::drain_deferred_jobs();
                }
                return Err(LxError::EAGAIN);
            }
            // If no endpoint, we can't send SOCK_DGRAM (no destination MAC).
            return Err(LxError::EINVAL);
        }

        // Do not call full poll_ifaces() here — edhcpc blocks inside send() while
        // waiting for DHCPOFFER; a heavy poll (link bringup + full RX drain) looks hung.
        for _ in 0..16 {
            match dev.send(data) {
                Ok(n) => {
                    kernel_hal::deferred_job::drain_deferred_jobs();
                    return Ok(n);
                }
                Err(_) => {
                    kernel_hal::deferred_job::drain_deferred_jobs();
                }
            }
        }
        Err(LxError::EAGAIN)
    }

    async fn connect(&self, _endpoint: Endpoint) -> SysResult {
        Err(LxError::EINVAL)
    }

    fn bind(&self, endpoint: Endpoint) -> SysResult {
        if let Endpoint::LinkLevel(ll) = endpoint {
            *self.inner.ifindex.lock() = ll.interface_index as u32;
            let proto = ll.protocol;
            *self.inner.protocol.lock() = proto;
            info!(
                "PacketSocket: bound to ifindex {}, proto (host)={:#x}",
                ll.interface_index, proto
            );
            Ok(0)
        } else {
            Err(LxError::EINVAL)
        }
    }

    fn endpoint(&self) -> Option<Endpoint> {
        Some(Endpoint::LinkLevel(LinkLevelEndpoint::new(
            *self.inner.ifindex.lock() as usize,
        )))
    }

    fn remote_endpoint(&self) -> Option<Endpoint> {
        None
    }

    fn setsockopt(&self, _level: usize, _opt: usize, _data: &[u8]) -> SysResult {
        Ok(0)
    }

    fn poll(&self, _events: PollEvents) -> (bool, bool, bool) {
        kernel_hal::deferred_job::drain_deferred_jobs();
        let ifindex = *self.inner.ifindex.lock();
        if ifindex > 0 {
            if let Ok(net) = crate::net::iface_by_linux_ifindex(ifindex) {
                crate::net::netdev_drain_rx(net.as_ref());
            }
        } else {
            for net in get_net_device().iter() {
                if net.get_ifname() != "loopback" {
                    crate::net::netdev_drain_rx(net.as_ref());
                }
            }
        }
        // Light NIC poll only — poll_ifaces() runs route/ARP sync + full link bringup
        // and stalls udhcpc select() while waiting for DHCPOFFER.
        if ifindex > 0 {
            if let Ok(net) = crate::net::iface_by_linux_ifindex(ifindex) {
                let _ = net.poll();
            }
        } else {
            for net in get_net_device().iter() {
                if net.get_ifname() != "loopback" {
                    let _ = net.poll();
                }
            }
        }
        let readable = !self.inner.packet_queue.lock().is_empty();
        let dev = if ifindex > 0 {
            crate::net::iface_by_linux_ifindex(ifindex).ok()
        } else {
            get_net_device()
                .into_iter()
                .find(|n| n.get_ifname() != "loopback")
        };
        let writable = dev.as_ref().is_some_and(|d| d.can_send());
        (readable, writable, false)
    }

    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> SysResult {
        trace!(
            "PacketSocket: ioctl request={:#x}, arg1={:#x}",
            request,
            arg1
        );
        handle_net_ioctl(request, arg1, arg2, arg3, false)
    }

    fn socket_type(&self) -> Option<SocketType> {
        Some(self.inner.socket_type)
    }
}

zircon_object::impl_kobject!(PacketSocketState);

#[async_trait]
impl FileLike for PacketSocketState {
    fn flags(&self) -> OpenFlags {
        *self.inner.flags.lock()
    }

    fn set_flags(&self, f: OpenFlags) -> LxResult {
        let mut flags = self.inner.flags.lock();
        flags.set(OpenFlags::APPEND, f.contains(OpenFlags::APPEND));
        flags.set(OpenFlags::NON_BLOCK, f.contains(OpenFlags::NON_BLOCK));
        flags.set(OpenFlags::CLOEXEC, f.contains(OpenFlags::CLOEXEC));
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        let inner = self.inner.clone();
        let state = Arc::new(Self {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            inner,
        });
        register_fd(&state.inner, &state);
        state
    }

    async fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        Socket::read(self, buf).await.0
    }

    async fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> LxResult<usize> {
        Err(LxError::ESPIPE)
    }

    fn write(&self, buf: &[u8]) -> LxResult<usize> {
        Socket::write(self, buf, None)
    }

    fn poll(&self, events: PollEvents) -> LxResult<PollStatus> {
        let (read, write, error) = Socket::poll(self, events);
        Ok(PollStatus { read, write, error })
    }

    async fn async_poll(&self, events: PollEvents) -> LxResult<PollStatus> {
        // Fast path: drain deferred jobs so any IRQ-enqueued packets are visible.
        kernel_hal::deferred_job::drain_deferred_jobs();
        let (read, write, error) = Socket::poll(self, events);

        // If the caller is waiting for readability (e.g. select/epoll in udhcpc)
        // and the queue is currently empty, sleep briefly and re-poll.
        // Without this, select() returns immediately with read=false every 5 ms
        // (from the executor tick), burning CPU and missing DHCPOFFER/DHCPACK
        // windows on slow links.
        if events.contains(PollEvents::IN) && !read && !error {
            kernel_hal::net::NetRxOrTimeoutFuture::new(5).await;
            kernel_hal::deferred_job::drain_deferred_jobs();
            let (read2, write2, error2) = Socket::poll(self, events);
            return Ok(PollStatus {
                read: read2,
                write: write2,
                error: error2,
            });
        }
        Ok(PollStatus { read, write, error })
    }

    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<usize> {
        Socket::ioctl(self, request, arg1, arg2, arg3)
    }

    fn as_socket(&self) -> LxResult<&dyn Socket> {
        Ok(self)
    }
}
