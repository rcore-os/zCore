//! Minimal `perf_event_open(2)` support: software CPU-clock sampling.
//!
//! This is **not** a hardware-PMU implementation. It implements enough of the
//! perf ring-buffer ABI for `perf top` / `perf record` to run and display a
//! live, sampled profile of user-space code:
//!
//! - `perf_event_open` returns a working fd (see [`sys_perf_event_open`]).
//! - `mmap`-ing the fd returns the ring buffer (control page + data pages).
//! - `ioctl(ENABLE/DISABLE/RESET/...)` toggles sampling.
//! - the timer-interrupt return path calls [`sample_user`], which appends a
//!   `PERF_RECORD_SAMPLE` (honouring the event's `sample_type`) into every
//!   enabled, matching ring buffer.
//!
//! Records are encoded byte-exactly per the kernel ABI so unmodified `perf`
//! parses them. Hardware events are accepted but sampled with the same
//! timer-driven software clock (there is no real PMU here), which is the
//! standard fallback when no PMU is available.

use super::*;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use lock::Mutex;
use zircon_object::object::*;
use zircon_object::vm::{pages, VmObject};

use crate::sync::{Event, EventBus};

const PAGE_SIZE: usize = 4096;

// ---- perf_event_attr field offsets (LP64 ABI) ----
const ATTR_OFF_TYPE: usize = 0; // u32
const ATTR_OFF_CONFIG: usize = 8; // u64
const ATTR_OFF_SAMPLE_PERIOD: usize = 16; // u64 (sample_period | sample_freq)
const ATTR_OFF_SAMPLE_TYPE: usize = 24; // u64
const ATTR_OFF_READ_FORMAT: usize = 32; // u64
const ATTR_OFF_FLAGS: usize = 40; // u64 bitfield

// ---- attr flag bits (within the u64 at ATTR_OFF_FLAGS) ----
const ATTR_FLAG_DISABLED: u64 = 1 << 0;
const ATTR_FLAG_FREQ: u64 = 1 << 10;

// ---- PERF_SAMPLE_* bits (sample_type) ----
const PERF_SAMPLE_IP: u64 = 1 << 0;
const PERF_SAMPLE_TID: u64 = 1 << 1;
const PERF_SAMPLE_TIME: u64 = 1 << 2;
const PERF_SAMPLE_ADDR: u64 = 1 << 3;
const PERF_SAMPLE_READ: u64 = 1 << 4;
const PERF_SAMPLE_CALLCHAIN: u64 = 1 << 5;
const PERF_SAMPLE_ID: u64 = 1 << 6;
const PERF_SAMPLE_CPU: u64 = 1 << 7;
const PERF_SAMPLE_PERIOD: u64 = 1 << 8;
const PERF_SAMPLE_STREAM_ID: u64 = 1 << 9;
const PERF_SAMPLE_RAW: u64 = 1 << 10;
const PERF_SAMPLE_BRANCH_STACK: u64 = 1 << 11;
const PERF_SAMPLE_REGS_USER: u64 = 1 << 12;
const PERF_SAMPLE_STACK_USER: u64 = 1 << 13;
const PERF_SAMPLE_WEIGHT: u64 = 1 << 14;
const PERF_SAMPLE_DATA_SRC: u64 = 1 << 15;
const PERF_SAMPLE_IDENTIFIER: u64 = 1 << 16;
const PERF_SAMPLE_TRANSACTION: u64 = 1 << 17;
const PERF_SAMPLE_REGS_INTR: u64 = 1 << 18;
const PERF_SAMPLE_PHYS_ADDR: u64 = 1 << 19;

// ---- read_format bits ----
const PERF_FORMAT_TOTAL_TIME_ENABLED: u64 = 1 << 0;
const PERF_FORMAT_TOTAL_TIME_RUNNING: u64 = 1 << 1;
const PERF_FORMAT_ID: u64 = 1 << 2;

// ---- record types / misc ----
const PERF_RECORD_SAMPLE: u32 = 9;
const PERF_RECORD_MISC_USER: u16 = 2;

// ---- ioctl numbers (magic '$' = 0x24) ----
const PERF_EVENT_IOC_ENABLE: usize = 0x2400;
const PERF_EVENT_IOC_DISABLE: usize = 0x2401;
const PERF_EVENT_IOC_REFRESH: usize = 0x2402;
const PERF_EVENT_IOC_RESET: usize = 0x2403;
const PERF_EVENT_IOC_PERIOD: usize = 0x4008_2404;
const PERF_EVENT_IOC_SET_OUTPUT: usize = 0x2405;
const PERF_EVENT_IOC_SET_FILTER: usize = 0x4008_2406;
const PERF_EVENT_IOC_ID: usize = 0x8008_2407;

// ---- control-page (perf_event_mmap_page) field offsets ----
const PC_VERSION: usize = 0; // u32
const PC_DATA_HEAD: usize = 1024; // u64
const PC_DATA_TAIL: usize = 1032; // u64
const PC_DATA_OFFSET: usize = 1040; // u64
const PC_DATA_SIZE: usize = 1048; // u64

/// Next event id, used as the perf `id` / `stream_id`.
static NEXT_ID: Mutex<u64> = Mutex::new(1);

fn alloc_id() -> u64 {
    let mut g = NEXT_ID.lock();
    let id = *g;
    *g += 1;
    id
}

/// Global registry of live perf events, consulted by the sampler.
static PERF_EVENTS: Mutex<Vec<Weak<PerfEvent>>> = Mutex::new(Vec::new());

/// Set once any perf event is enabled, so the hot timer path can cheaply skip
/// the registry lock when nothing is profiling.
static ANY_ENABLED: AtomicBool = AtomicBool::new(false);

/// The mmap ring buffer of one perf event.
struct Ring {
    vmo: Arc<VmObject>,
    /// Size in bytes of the data region (excludes the control page).
    data_size: usize,
}

struct PerfInner {
    /// `type` field of `perf_event_attr` (PERF_TYPE_*).
    _type: u32,
    /// `config` field (which event).
    _config: u64,
    sample_type: u64,
    read_format: u64,
    /// Effective sampling period in "events" (software clock ticks here). When
    /// the attr asked for a frequency we still sample once per timer tick and
    /// report a period of 1; perf re-derives a rate from the timestamps.
    period: u64,
    id: u64,
    enabled: bool,
    /// Free-running write counter (bytes ever written to the data region).
    data_head: u64,
    /// Accumulated event count, returned by `read(2)`.
    count: u64,
    /// Dropped samples because the ring was full (reported as LOST is TODO).
    lost: u64,
    ring: Option<Ring>,
}

/// A single `perf_event_open` file descriptor.
pub struct PerfEvent {
    base: KObjectBase,
    /// CPU this event is bound to, or `-1` for any CPU.
    cpu: i32,
    /// PID this event profiles, or `-1` for all processes.
    pid: i32,
    flags: OpenFlags,
    eventbus: Arc<Mutex<EventBus>>,
    inner: Arc<Mutex<PerfInner>>,
}

impl_kobject!(PerfEvent);

impl PerfEvent {
    /// Build an event from a raw `perf_event_attr` byte image.
    pub fn new(attr: &[u8], pid: i32, cpu: i32, flags: OpenFlags) -> Arc<Self> {
        let rd_u32 = |off: usize| -> u32 {
            let mut b = [0u8; 4];
            if off + 4 <= attr.len() {
                b.copy_from_slice(&attr[off..off + 4]);
            }
            u32::from_ne_bytes(b)
        };
        let rd_u64 = |off: usize| -> u64 {
            let mut b = [0u8; 8];
            if off + 8 <= attr.len() {
                b.copy_from_slice(&attr[off..off + 8]);
            }
            u64::from_ne_bytes(b)
        };

        let type_ = rd_u32(ATTR_OFF_TYPE);
        let config = rd_u64(ATTR_OFF_CONFIG);
        let sample_type = rd_u64(ATTR_OFF_SAMPLE_TYPE);
        let read_format = rd_u64(ATTR_OFF_READ_FORMAT);
        let attr_flags = rd_u64(ATTR_OFF_FLAGS);
        let sample_period = rd_u64(ATTR_OFF_SAMPLE_PERIOD);
        let freq_mode = attr_flags & ATTR_FLAG_FREQ != 0;
        // In freq mode the field is a target Hz; we still sample per tick, so a
        // reported period of 1 keeps perf's accounting consistent. In period
        // mode, report the configured period (min 1).
        let period = if freq_mode { 1 } else { sample_period.max(1) };
        let enabled = attr_flags & ATTR_FLAG_DISABLED == 0;

        let event = Arc::new(PerfEvent {
            base: KObjectBase::new(),
            cpu,
            pid,
            flags,
            eventbus: EventBus::new(),
            inner: Arc::new(Mutex::new(PerfInner {
                _type: type_,
                _config: config,
                sample_type,
                read_format,
                period,
                id: alloc_id(),
                enabled,
                data_head: 0,
                count: 0,
                lost: 0,
                ring: None,
            })),
        });
        if enabled {
            ANY_ENABLED.store(true, Ordering::Relaxed);
        }
        register(&event);
        event
    }

    /// Append a `PERF_RECORD_SAMPLE` for an interrupted user instruction.
    fn record_sample(&self, pid: i32, tid: i32, cpu: u32, ip: u64, time_ns: u64) {
        let mut inner = self.inner.lock();
        if !inner.enabled || inner.ring.is_none() {
            return;
        }
        let period = inner.period;
        inner.count = inner.count.wrapping_add(period);

        // Encode the sample body in the canonical field order, emitting exactly
        // the fields requested in `sample_type` so unmodified perf parses it.
        let st = inner.sample_type;
        let mut body: Vec<u8> = Vec::with_capacity(64);
        let push_u64 = |v: &mut Vec<u8>, x: u64| v.extend_from_slice(&x.to_ne_bytes());
        let push_u32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_ne_bytes());

        if st & PERF_SAMPLE_IDENTIFIER != 0 {
            push_u64(&mut body, inner.id);
        }
        if st & PERF_SAMPLE_IP != 0 {
            push_u64(&mut body, ip);
        }
        if st & PERF_SAMPLE_TID != 0 {
            push_u32(&mut body, pid as u32);
            push_u32(&mut body, tid as u32);
        }
        if st & PERF_SAMPLE_TIME != 0 {
            push_u64(&mut body, time_ns);
        }
        if st & PERF_SAMPLE_ADDR != 0 {
            push_u64(&mut body, 0);
        }
        if st & PERF_SAMPLE_ID != 0 {
            push_u64(&mut body, inner.id);
        }
        if st & PERF_SAMPLE_STREAM_ID != 0 {
            push_u64(&mut body, inner.id);
        }
        if st & PERF_SAMPLE_CPU != 0 {
            push_u32(&mut body, cpu);
            push_u32(&mut body, 0);
        }
        if st & PERF_SAMPLE_PERIOD != 0 {
            push_u64(&mut body, period);
        }
        if st & PERF_SAMPLE_READ != 0 {
            // Non-group read_format only.
            push_u64(&mut body, inner.count);
            if inner.read_format & PERF_FORMAT_TOTAL_TIME_ENABLED != 0 {
                push_u64(&mut body, time_ns);
            }
            if inner.read_format & PERF_FORMAT_TOTAL_TIME_RUNNING != 0 {
                push_u64(&mut body, time_ns);
            }
            if inner.read_format & PERF_FORMAT_ID != 0 {
                push_u64(&mut body, inner.id);
            }
        }
        if st & PERF_SAMPLE_CALLCHAIN != 0 {
            push_u64(&mut body, 0); // nr = 0
        }
        if st & PERF_SAMPLE_RAW != 0 {
            push_u32(&mut body, 0); // raw size = 0
            push_u32(&mut body, 0); // pad to 8 bytes
        }
        if st & PERF_SAMPLE_BRANCH_STACK != 0 {
            push_u64(&mut body, 0); // nr = 0
        }
        if st & PERF_SAMPLE_REGS_USER != 0 {
            push_u64(&mut body, 0); // abi = NONE (no regs follow)
        }
        if st & PERF_SAMPLE_STACK_USER != 0 {
            push_u64(&mut body, 0); // size = 0 (no data / dyn_size)
        }
        if st & PERF_SAMPLE_WEIGHT != 0 {
            push_u64(&mut body, 0);
        }
        if st & PERF_SAMPLE_DATA_SRC != 0 {
            push_u64(&mut body, 0);
        }
        if st & PERF_SAMPLE_TRANSACTION != 0 {
            push_u64(&mut body, 0);
        }
        if st & PERF_SAMPLE_REGS_INTR != 0 {
            push_u64(&mut body, 0); // abi = NONE
        }
        if st & PERF_SAMPLE_PHYS_ADDR != 0 {
            push_u64(&mut body, 0);
        }
        // Pad the whole record to an 8-byte boundary.
        while !body.len().is_multiple_of(8) {
            body.push(0);
        }

        let total = 8 + body.len(); // perf_event_header is 8 bytes
        let mut record: Vec<u8> = Vec::with_capacity(total);
        record.extend_from_slice(&PERF_RECORD_SAMPLE.to_ne_bytes());
        record.extend_from_slice(&PERF_RECORD_MISC_USER.to_ne_bytes());
        record.extend_from_slice(&(total as u16).to_ne_bytes());
        record.extend_from_slice(&body);

        self.ring_write(&mut inner, &record);
        drop(inner);
        // Wake any poller waiting for data.
        self.eventbus.lock().set(Event::READABLE);
    }

    /// Write a fully-formed record into the data region, wrapping as needed,
    /// then publish the new `data_head`. Drops the sample if the consumer
    /// (perf) has not made room.
    fn ring_write(&self, inner: &mut PerfInner, record: &[u8]) {
        let Some(ring) = inner.ring.as_ref() else {
            return;
        };
        let data_size = ring.data_size;
        if record.is_empty() || record.len() > data_size {
            return;
        }
        // Read the consumer tail from the control page.
        let mut tail_b = [0u8; 8];
        let _ = ring.vmo.read(PC_DATA_TAIL, &mut tail_b);
        let data_tail = u64::from_ne_bytes(tail_b);
        let head = inner.data_head;
        // Available space in a non-overwrite ring.
        let used = head.wrapping_sub(data_tail);
        if used + record.len() as u64 > data_size as u64 {
            inner.lost = inner.lost.wrapping_add(1);
            return;
        }
        let pos = (head % data_size as u64) as usize;
        let base = PAGE_SIZE; // data region starts after the control page
        if pos + record.len() <= data_size {
            let _ = ring.vmo.write(base + pos, record);
        } else {
            let first = data_size - pos;
            let _ = ring.vmo.write(base + pos, &record[..first]);
            let _ = ring.vmo.write(base, &record[first..]);
        }
        let new_head = head.wrapping_add(record.len() as u64);
        inner.data_head = new_head;
        // Publish head last so the consumer never sees a head past unwritten data.
        let _ = ring.vmo.write(PC_DATA_HEAD, &new_head.to_ne_bytes());
    }

    fn has_data(&self) -> bool {
        let inner = self.inner.lock();
        let Some(ring) = inner.ring.as_ref() else {
            return false;
        };
        let mut tail_b = [0u8; 8];
        let _ = ring.vmo.read(PC_DATA_TAIL, &mut tail_b);
        inner.data_head != u64::from_ne_bytes(tail_b)
    }
}

#[async_trait]
impl FileLike for PerfEvent {
    fn flags(&self) -> OpenFlags {
        self.flags
    }

    fn set_flags(&self, _f: OpenFlags) -> LxResult {
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        // Share the same underlying event/ring (the original stays registered
        // with the sampler; both fds observe the same samples).
        Arc::new(PerfEvent {
            base: KObjectBase::new(),
            cpu: self.cpu,
            pid: self.pid,
            flags: self.flags,
            eventbus: self.eventbus.clone(),
            inner: self.inner.clone(),
        })
    }

    async fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        // Non-mmap read returns the accumulated count (optionally enabled/
        // running/id per read_format), like a counting event.
        let inner = self.inner.lock();
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&inner.count.to_ne_bytes());
        if inner.read_format & PERF_FORMAT_TOTAL_TIME_ENABLED != 0 {
            out.extend_from_slice(&0u64.to_ne_bytes());
        }
        if inner.read_format & PERF_FORMAT_TOTAL_TIME_RUNNING != 0 {
            out.extend_from_slice(&0u64.to_ne_bytes());
        }
        if inner.read_format & PERF_FORMAT_ID != 0 {
            out.extend_from_slice(&inner.id.to_ne_bytes());
        }
        let n = out.len().min(buf.len());
        buf[..n].copy_from_slice(&out[..n]);
        Ok(n)
    }

    fn write(&self, _buf: &[u8]) -> LxResult<usize> {
        Err(LxError::EINVAL)
    }

    async fn read_at(&self, _offset: u64, buf: &mut [u8]) -> LxResult<usize> {
        self.read(buf).await
    }

    fn poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        Ok(PollStatus {
            read: self.has_data(),
            write: false,
            error: false,
        })
    }

    async fn async_poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        loop {
            if self.has_data() {
                return Ok(PollStatus {
                    read: true,
                    write: false,
                    error: false,
                });
            }
            let bus = self.eventbus.clone();
            crate::sync::wait_for_event(bus, Event::READABLE).await;
        }
    }

    fn ioctl(&self, request: usize, arg1: usize, _arg2: usize, _arg3: usize) -> LxResult<usize> {
        match request {
            PERF_EVENT_IOC_ENABLE => {
                self.inner.lock().enabled = true;
                ANY_ENABLED.store(true, Ordering::Relaxed);
                Ok(0)
            }
            PERF_EVENT_IOC_DISABLE => {
                self.inner.lock().enabled = false;
                Ok(0)
            }
            PERF_EVENT_IOC_RESET => {
                let mut inner = self.inner.lock();
                inner.count = 0;
                Ok(0)
            }
            PERF_EVENT_IOC_REFRESH => {
                // arg is a refresh count; treat as enable.
                self.inner.lock().enabled = true;
                ANY_ENABLED.store(true, Ordering::Relaxed);
                Ok(0)
            }
            PERF_EVENT_IOC_PERIOD => {
                // arg1 points at a u64 new period in user memory; best-effort.
                if arg1 != 0 {
                    let ptr = kernel_hal::user::UserInPtr::<u64>::from(arg1);
                    if let Ok(p) = ptr.read() {
                        self.inner.lock().period = p.max(1);
                    }
                }
                Ok(0)
            }
            PERF_EVENT_IOC_ID => {
                if arg1 != 0 {
                    let id = self.inner.lock().id;
                    let mut ptr = kernel_hal::user::UserOutPtr::<u64>::from(arg1);
                    let _ = ptr.write(id);
                }
                Ok(0)
            }
            // Grouping / output redirection / filters are accepted as no-ops so
            // perf does not bail; samples still land in this event's own ring.
            PERF_EVENT_IOC_SET_OUTPUT | PERF_EVENT_IOC_SET_FILTER => Ok(0),
            _ => Err(LxError::ENOTTY),
        }
    }

    fn get_vmo(&self, offset: usize, len: usize) -> LxResult<Arc<VmObject>> {
        // perf maps the ring buffer at file offset 0: one control page followed
        // by `2^n` data pages. Create it on first mmap and cache it so the
        // sampler and the user mapping share the same physical frames.
        if offset != 0 || len < 2 * PAGE_SIZE {
            return Err(LxError::EINVAL);
        }
        let mut inner = self.inner.lock();
        if let Some(ring) = inner.ring.as_ref() {
            return Ok(ring.vmo.clone());
        }
        let total_pages = pages(len);
        let data_size = (total_pages - 1) * PAGE_SIZE;
        let vmo = VmObject::new_paged(total_pages);
        // Initialise the control page so perf finds a sane header.
        let _ = vmo.write(PC_VERSION, &0u32.to_ne_bytes());
        let _ = vmo.write(PC_DATA_HEAD, &0u64.to_ne_bytes());
        let _ = vmo.write(PC_DATA_TAIL, &0u64.to_ne_bytes());
        let _ = vmo.write(PC_DATA_OFFSET, &(PAGE_SIZE as u64).to_ne_bytes());
        let _ = vmo.write(PC_DATA_SIZE, &(data_size as u64).to_ne_bytes());
        inner.ring = Some(Ring {
            vmo: vmo.clone(),
            data_size,
        });
        Ok(vmo)
    }
}

fn register(event: &Arc<PerfEvent>) {
    let mut list = PERF_EVENTS.lock();
    list.retain(|w| w.strong_count() > 0);
    list.push(Arc::downgrade(event));
}

/// Record a user-space sample on every enabled perf event that matches the
/// given CPU and PID. Called from the timer-interrupt return path.
///
/// Cheap and non-blocking: returns immediately when nothing is profiling.
pub fn sample_user(pid: i32, tid: i32, cpu: u32, ip: u64) {
    if !ANY_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let time_ns = kernel_hal::timer::timer_now().as_nanos() as u64;
    // Snapshot the matching events, then drop the registry lock before touching
    // each event's own lock to avoid holding two locks at once.
    let events: Vec<Arc<PerfEvent>> = {
        let list = PERF_EVENTS.lock();
        list.iter().filter_map(|w| w.upgrade()).collect()
    };
    let mut any_enabled = false;
    for ev in events {
        let (matches, enabled) = {
            let inner = ev.inner.lock();
            any_enabled |= inner.enabled;
            (
                (ev.cpu < 0 || ev.cpu as u32 == cpu) && (ev.pid < 0 || ev.pid == pid),
                inner.enabled,
            )
        };
        if matches && enabled {
            ev.record_sample(pid, tid, cpu, ip, time_ns);
        }
    }
    if !any_enabled {
        ANY_ENABLED.store(false, Ordering::Relaxed);
    }
}
