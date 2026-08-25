//! `/dev/snd/*` — native ALSA kernel ABI over an [`AudioScheme`] device.
//!
//! This is what stock alsa-lib (`aplay`, SDL, mpg123, anything using
//! `snd_pcm_*`) talks to: one `controlC<card>` + `pcmC<card>D0p` pair per HDA
//! controller. The implementation targets alsa-lib's `hw` plugin in its
//! **RW-interleaved + SYNC_PTR** mode:
//!
//! * `SNDRV_PCM_IOCTL_HW_REFINE/HW_PARAMS` constrain to what the driver ring
//!   does natively — S16LE, stereo, the discrete HDA rate set — and alsa-lib's
//!   `plug` layer (routed in by the shipped `/etc/asound.conf`) converts
//!   everything else in userspace.
//! * Data moves through `SNDRV_PCM_IOCTL_WRITEI_FRAMES`; the status/control
//!   pages are *not* mmap-able here, which alsa-lib detects and transparently
//!   falls back to `SNDRV_PCM_IOCTL_SYNC_PTR` — implemented with Linux's exact
//!   flag semantics.
//! * No capture, no mmap data access, no pause/resume: the `info` flags and
//!   refine masks never advertise them, so clients don't try.
//!
//! Ioctls are matched on the `_IOC` type+nr bytes only (not the size bits):
//! the struct layouts here mirror the x86_64 uapi, but matching the full cmd
//! word would break the moment alsa-lib is built against a slightly newer
//! header that grew a reserved field.

#![allow(unsafe_code)]

use alloc::sync::Arc;
use core::any::Any;

use kernel_hal::drivers::scheme::AudioScheme;
use lock::Mutex;
use rcore_fs::vfs::*;
use rcore_fs_devfs::DevFS;

// ── uapi mirror (x86_64) ────────────────────────────────────────────────────

const SNDRV_PCM_VERSION: i32 = 0x0002_000e; // 2.0.14
const SNDRV_CTL_VERSION: i32 = 0x0002_0007;

// snd_pcm_state_t
const STATE_SETUP: i32 = 1;
const STATE_PREPARED: i32 = 2;
const STATE_RUNNING: i32 = 3;
const STATE_OPEN: i32 = 0;

// snd_pcm_hw_param indexes.
const PAR_ACCESS: usize = 0;
const PAR_FORMAT: usize = 1;
const PAR_SUBFORMAT: usize = 2;
// Interval params, biased by FIRST_INTERVAL = 8 when indexing `intervals`.
const PAR_SAMPLE_BITS: usize = 8;
const PAR_FRAME_BITS: usize = 9;
const PAR_CHANNELS: usize = 10;
const PAR_RATE: usize = 11;
const PAR_PERIOD_TIME: usize = 12;
const PAR_PERIOD_SIZE: usize = 13;
const PAR_PERIOD_BYTES: usize = 14;
const PAR_PERIODS: usize = 15;
const PAR_BUFFER_TIME: usize = 16;
const PAR_BUFFER_SIZE: usize = 17;
const PAR_BUFFER_BYTES: usize = 18;
const PAR_TICK_TIME: usize = 19;

const ACCESS_RW_INTERLEAVED: u32 = 3;
const FORMAT_S16_LE: u32 = 2;
const SUBFORMAT_STD: u32 = 0;

const INFO_INTERLEAVED: u32 = 0x0000_0100;
const INFO_BLOCK_TRANSFER: u32 = 0x0001_0000;

const SYNC_PTR_HWSYNC: u32 = 1;
const SYNC_PTR_APPL: u32 = 2;
const SYNC_PTR_AVAIL_MIN: u32 = 4;

const INTERVAL_OPENMIN: u32 = 1 << 0;
const INTERVAL_OPENMAX: u32 = 1 << 1;
const INTERVAL_INTEGER: u32 = 1 << 2;
const INTERVAL_EMPTY: u32 = 1 << 3;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SndInterval {
    min: u32,
    max: u32,
    flags: u32, // openmin | openmax<<1 | integer<<2 | empty<<3
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SndMask {
    bits: [u32; 8],
}

#[repr(C)]
struct SndPcmHwParams {
    flags: u32,
    masks: [SndMask; 3],
    mres: [SndMask; 5],
    intervals: [SndInterval; 12],
    ires: [SndInterval; 9],
    rmask: u32,
    cmask: u32,
    info: u32,
    msbits: u32,
    rate_num: u32,
    rate_den: u32,
    fifo_size: u64,
    reserved: [u8; 64],
}

#[repr(C)]
struct SndPcmSwParams {
    tstamp_mode: i32,
    period_step: u32,
    sleep_min: u32,
    avail_min: u64,
    xfer_align: u64,
    start_threshold: u64,
    stop_threshold: u64,
    silence_threshold: u64,
    silence_size: u64,
    boundary: u64,
    proto: u32,
    tstamp_type: u32,
    reserved: [u8; 56],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

#[repr(C)]
struct SndPcmStatus {
    state: i32,
    _pad0: i32,
    trigger_tstamp: Timespec,
    tstamp: Timespec,
    appl_ptr: u64,
    hw_ptr: u64,
    delay: i64,
    avail: u64,
    avail_max: u64,
    overrange: u64,
    suspended_state: i32,
    audio_tstamp_data: u32,
    audio_tstamp: Timespec,
    driver_tstamp: Timespec,
    audio_tstamp_accuracy: u32,
    reserved: [u8; 20],
}

#[repr(C)]
struct SndPcmMmapStatus {
    state: i32,
    _pad1: i32,
    hw_ptr: u64,
    tstamp: Timespec,
    suspended_state: i32,
    _pad2: i32,
    audio_tstamp: Timespec,
}

#[repr(C)]
struct SndPcmMmapControl {
    appl_ptr: u64,
    avail_min: u64,
}

#[repr(C)]
struct SndPcmSyncPtr {
    flags: u32,
    _pad: u32,
    status: SndPcmMmapStatus, // union with u8[64]
    _spad: [u8; 64 - core::mem::size_of::<SndPcmMmapStatus>()],
    control: SndPcmMmapControl, // union with u8[64]
    _cpad: [u8; 64 - core::mem::size_of::<SndPcmMmapControl>()],
}

#[repr(C)]
struct SndXferI {
    result: i64,
    buf: u64,
    frames: u64,
}

#[repr(C)]
struct SndPcmInfo {
    device: u32,
    subdevice: u32,
    stream: i32,
    card: i32,
    id: [u8; 64],
    name: [u8; 80],
    subname: [u8; 32],
    dev_class: i32,
    dev_subclass: i32,
    subdevices_count: u32,
    subdevices_avail: u32,
    sync: [u8; 16],
    reserved: [u8; 64],
}

#[repr(C)]
struct SndCtlCardInfo {
    card: i32,
    _pad: i32,
    id: [u8; 16],
    driver: [u8; 16],
    name: [u8; 32],
    longname: [u8; 80],
    reserved_: [u8; 16],
    mixername: [u8; 80],
    components: [u8; 128],
}

fn fill_cstr(dst: &mut [u8], s: &str) {
    let n = s.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&s.as_bytes()[..n]);
    dst[n..].fill(0);
}

/// The discrete rates the HDA driver encodes (see `stream_format`).
const RATES: [u32; 11] = [
    8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000,
];

const BYTES_PER_FRAME: u64 = 4; // S16LE stereo

// ── PCM device node ─────────────────────────────────────────────────────────

struct PcmState {
    state: i32,
    rate: u32,
    buffer_size: u64, // frames
    period_size: u64, // frames
    boundary: u64,
    appl_ptr: u64,
    avail_min: u64,
}

pub struct PcmDev {
    audio: Arc<dyn AudioScheme>,
    card: usize,
    inode_id: usize,
    st: Mutex<PcmState>,
}

impl PcmDev {
    pub fn new(audio: Arc<dyn AudioScheme>, card: usize) -> Self {
        PcmDev {
            audio,
            card,
            inode_id: DevFS::new_inode_id(),
            st: Mutex::new(PcmState {
                state: STATE_OPEN,
                rate: 48000,
                buffer_size: 16384,
                period_size: 1024,
                boundary: 0x4000_0000_0000_0000,
                appl_ptr: 0,
                avail_min: 1024,
            }),
        }
    }

    fn ring_frames(&self) -> u64 {
        self.audio.buffer_bytes() as u64 / BYTES_PER_FRAME
    }

    fn queued_frames(&self) -> u64 {
        self.audio.queued_bytes() as u64 / BYTES_PER_FRAME
    }

    /// Frames the client may write within its negotiated buffer.
    fn avail(&self, st: &PcmState) -> u64 {
        st.buffer_size.saturating_sub(self.queued_frames())
    }

    fn hw_ptr(&self, st: &PcmState) -> u64 {
        (st.appl_ptr + st.boundary - self.queued_frames()) % st.boundary
    }

    // ── hw_params refine ────────────────────────────────────────────────────

    fn clamp_interval(iv: &mut SndInterval, lo: u32, hi: u32, integer: bool) -> bool {
        if iv.min < lo {
            iv.min = lo;
            iv.flags &= !INTERVAL_OPENMIN;
        }
        if iv.max > hi {
            iv.max = hi;
            iv.flags &= !INTERVAL_OPENMAX;
        }
        if integer {
            iv.flags |= INTERVAL_INTEGER;
        }
        if iv.min > iv.max {
            iv.flags |= INTERVAL_EMPTY;
            return false;
        }
        true
    }

    fn set_interval(iv: &mut SndInterval, v: u32) -> bool {
        Self::clamp_interval(iv, v, v, true)
    }

    /// Constrain a hw_params request to what the hardware path does. Returns
    /// `false` when any parameter became empty (→ EINVAL, which alsa-lib's
    /// `*_near` searches use to home in on supported values).
    fn refine(&self, p: &mut SndPcmHwParams) -> bool {
        let mut ok = true;

        p.masks[PAR_ACCESS].bits[0] &= 1 << ACCESS_RW_INTERLEAVED;
        p.masks[PAR_FORMAT].bits[0] &= 1 << FORMAT_S16_LE;
        p.masks[PAR_FORMAT].bits[1] = 0;
        p.masks[PAR_SUBFORMAT].bits[0] &= 1 << SUBFORMAT_STD;
        for m in &mut p.masks {
            for w in &mut m.bits[2..] {
                *w = 0;
            }
        }
        ok &= p.masks[PAR_ACCESS].bits[0] != 0;
        ok &= p.masks[PAR_FORMAT].bits[0] != 0;
        ok &= p.masks[PAR_SUBFORMAT].bits[0] != 0;

        let iv = &mut p.intervals;
        ok &= Self::set_interval(&mut iv[PAR_SAMPLE_BITS - 8], 16);
        ok &= Self::set_interval(&mut iv[PAR_FRAME_BITS - 8], 32);
        ok &= Self::set_interval(&mut iv[PAR_CHANNELS - 8], 2);

        // Rate: snap the interval to the discrete supported set.
        {
            let r = &mut iv[PAR_RATE - 8];
            let lo = RATES.iter().copied().find(|&x| x >= r.min);
            let hi = RATES.iter().rev().copied().find(|&x| x <= r.max);
            match (lo, hi) {
                (Some(lo), Some(hi)) if lo <= hi => {
                    ok &= Self::clamp_interval(r, lo, hi, true);
                }
                _ => {
                    r.flags |= INTERVAL_EMPTY;
                    ok = false;
                }
            }
        }

        let ring = self.ring_frames() as u32;
        ok &= Self::clamp_interval(&mut iv[PAR_PERIOD_SIZE - 8], 64, 8192, true);
        ok &= Self::clamp_interval(&mut iv[PAR_PERIOD_BYTES - 8], 64 * 4, 8192 * 4, true);
        ok &= Self::clamp_interval(&mut iv[PAR_PERIODS - 8], 2, 512, true);
        ok &= Self::clamp_interval(&mut iv[PAR_BUFFER_SIZE - 8], 128, ring, true);
        ok &= Self::clamp_interval(&mut iv[PAR_BUFFER_BYTES - 8], 128 * 4, ring * 4, true);

        // Times in µs, derived loosely from the rate/size bounds above.
        let rate_min = iv[PAR_RATE - 8].min.max(1);
        let rate_max = iv[PAR_RATE - 8].max.max(1);
        let pt_min = (64u64 * 1_000_000 / rate_max as u64) as u32;
        let pt_max = (8192u64 * 1_000_000 / rate_min as u64 + 1) as u32;
        ok &= Self::clamp_interval(&mut iv[PAR_PERIOD_TIME - 8], pt_min.max(1), pt_max, false);
        let bt_min = (128u64 * 1_000_000 / rate_max as u64) as u32;
        let bt_max = (ring as u64 * 1_000_000 / rate_min as u64 + 1) as u32;
        ok &= Self::clamp_interval(&mut iv[PAR_BUFFER_TIME - 8], bt_min.max(1), bt_max, false);
        ok &= Self::clamp_interval(&mut iv[PAR_TICK_TIME - 8], 0, 1_000_000, false);

        p.info = INFO_INTERLEAVED | INFO_BLOCK_TRANSFER;
        p.msbits = 16;
        p.rate_den = 1;
        p.rate_num = 0;
        p.fifo_size = 0;
        // Report everything as (potentially) changed; alsa-lib re-reads all.
        p.cmask = 0x000f_ff07;
        ok
    }

    /// Choose concrete values inside the (already refined) request.
    fn install(&self, p: &mut SndPcmHwParams) -> Result<()> {
        if !self.refine(p) {
            return Err(FsError::InvalidParam);
        }
        let iv = &p.intervals;

        // Rate: the smallest supported rate inside the refined interval (after
        // `refine` both ends are supported values).
        let rate = RATES
            .iter()
            .copied()
            .find(|&r| r >= iv[PAR_RATE - 8].min && r <= iv[PAR_RATE - 8].max)
            .ok_or(FsError::InvalidParam)?;

        let period = iv[PAR_PERIOD_SIZE - 8].min.max(64) as u64;
        let ring = self.ring_frames();
        let mut buffer = iv[PAR_BUFFER_SIZE - 8].min.max(128) as u64;
        // If the client left the buffer wide open, take something comfortable.
        if iv[PAR_BUFFER_SIZE - 8].max as u64 > buffer * 4 {
            buffer = (period * 8).clamp(buffer, iv[PAR_BUFFER_SIZE - 8].max as u64);
        }
        let buffer = buffer.min(ring);

        let (actual_rate, _ch) = self
            .audio
            .set_params(rate, 2)
            .map_err(|_| FsError::DeviceError)?;

        let mut st = self.st.lock();
        st.rate = actual_rate;
        st.period_size = period;
        st.buffer_size = buffer;
        st.appl_ptr = 0;
        // Same boundary algorithm as Linux + alsa-lib, so both sides agree.
        let mut boundary = buffer;
        while boundary < 0x4000_0000_0000_0000u64 {
            boundary *= 2;
        }
        st.boundary = boundary;
        st.state = STATE_SETUP;
        drop(st);

        // Report the concrete configuration back as exact singletons.
        let iv = &mut p.intervals;
        let _ = Self::set_interval(&mut iv[PAR_RATE - 8], actual_rate);
        let _ = Self::set_interval(&mut iv[PAR_PERIOD_SIZE - 8], period as u32);
        let _ = Self::set_interval(&mut iv[PAR_PERIOD_BYTES - 8], period as u32 * 4);
        let _ = Self::set_interval(&mut iv[PAR_BUFFER_SIZE - 8], buffer as u32);
        let _ = Self::set_interval(&mut iv[PAR_BUFFER_BYTES - 8], buffer as u32 * 4);
        let _ = Self::set_interval(
            &mut iv[PAR_PERIODS - 8],
            (buffer / period).max(1) as u32,
        );
        let pt = (period * 1_000_000 / actual_rate as u64) as u32;
        let _ = Self::clamp_interval(&mut iv[PAR_PERIOD_TIME - 8], pt, pt + 1, false);
        let bt = (buffer * 1_000_000 / actual_rate as u64) as u32;
        let _ = Self::clamp_interval(&mut iv[PAR_BUFFER_TIME - 8], bt, bt + 1, false);
        p.rate_num = actual_rate;
        p.rate_den = 1;
        Ok(())
    }

    /// Blocking interleaved write: the ALSA equivalent of the OSS node's
    /// spin-retry (the synchronous INode contract has no waker to park on).
    fn writei(&self, xfer: &mut SndXferI) -> Result<()> {
        {
            let st = self.st.lock();
            if st.state != STATE_PREPARED && st.state != STATE_RUNNING {
                return Err(FsError::InvalidParam);
            }
        }
        let total_bytes = (xfer.frames * BYTES_PER_FRAME) as usize;
        let src = xfer.buf as *const u8;
        let mut done = 0usize;
        let deadline_step = core::time::Duration::from_secs(4);
        let mut deadline = kernel_hal::timer::timer_now() + deadline_step;
        while done < total_bytes {
            // Respect the negotiated buffer size: never queue past it.
            let st_buffer = self.st.lock().buffer_size;
            let queued = self.queued_frames();
            let room_frames = st_buffer.saturating_sub(queued);
            let room = (room_frames * BYTES_PER_FRAME) as usize;
            let chunk = room.min(total_bytes - done);
            let n = if chunk >= BYTES_PER_FRAME as usize {
                let buf = unsafe { core::slice::from_raw_parts(src.add(done), chunk) };
                self.audio.write(buf).map_err(|_| FsError::DeviceError)?
            } else {
                0
            };
            if n > 0 {
                done += n;
                let mut st = self.st.lock();
                st.appl_ptr = (st.appl_ptr + n as u64 / BYTES_PER_FRAME) % st.boundary;
                st.state = STATE_RUNNING;
                deadline = kernel_hal::timer::timer_now() + deadline_step;
                continue;
            }
            if kernel_hal::timer::timer_now() >= deadline {
                warn!("[snd] pcmC{}D0p: writei stalled", self.card);
                break;
            }
            kernel_hal::deferred_job::drain_deferred_jobs();
            core::hint::spin_loop();
        }
        xfer.result = (done as u64 / BYTES_PER_FRAME) as i64;
        Ok(())
    }

    fn drain(&self) -> Result<()> {
        let deadline = kernel_hal::timer::timer_now()
            + core::time::Duration::from_secs(
                (self.audio.queued_bytes() as u64 / (48_000 * BYTES_PER_FRAME) + 2).min(10),
            );
        while self.audio.queued_bytes() > 0 {
            if kernel_hal::timer::timer_now() >= deadline {
                break;
            }
            kernel_hal::deferred_job::drain_deferred_jobs();
            core::hint::spin_loop();
        }
        self.st.lock().state = STATE_SETUP;
        Ok(())
    }

    fn fill_status(&self, s: &mut SndPcmStatus) {
        unsafe { core::ptr::write_bytes(s as *mut SndPcmStatus as *mut u8, 0, core::mem::size_of::<SndPcmStatus>()) };
        let st = self.st.lock();
        s.state = st.state;
        s.appl_ptr = st.appl_ptr;
        s.hw_ptr = self.hw_ptr(&st);
        s.delay = self.queued_frames() as i64;
        s.avail = self.avail(&st);
        s.avail_max = st.buffer_size;
        let now = kernel_hal::timer::timer_now();
        s.tstamp = Timespec {
            sec: now.as_secs() as i64,
            nsec: now.subsec_nanos() as i64,
        };
    }
}

impl INode for PcmDev {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }

    fn poll(&self) -> Result<PollStatus> {
        let st = self.st.lock();
        let avail = self.avail(&st);
        Ok(PollStatus {
            read: false,
            write: avail >= st.avail_min,
            error: false,
        })
    }

    fn io_control(&self, cmd: u32, data: usize) -> Result<usize> {
        let ty = (cmd >> 8) & 0xff;
        let nr = cmd & 0xff;
        if ty != b'A' as u32 {
            return Err(FsError::NotSupported);
        }
        match nr {
            0x00 => {
                // PVERSION
                unsafe { *(data as *mut i32) = SNDRV_PCM_VERSION };
                Ok(0)
            }
            0x01 => {
                // INFO
                let info = unsafe { &mut *(data as *mut SndPcmInfo) };
                unsafe {
                    core::ptr::write_bytes(info as *mut SndPcmInfo as *mut u8, 0, core::mem::size_of::<SndPcmInfo>())
                };
                info.device = 0;
                info.subdevice = 0;
                info.stream = 0; // playback
                info.card = self.card as i32;
                fill_cstr(&mut info.id, "Eclipse HDA");
                fill_cstr(&mut info.name, self.audio.name());
                fill_cstr(&mut info.subname, "subdevice #0");
                info.subdevices_count = 1;
                info.subdevices_avail = 1;
                Ok(0)
            }
            // TSTAMP / TTSTAMP / USER_PVERSION: accepted, ignored.
            0x02 | 0x03 | 0x04 => Ok(0),
            0x10 => {
                // HW_REFINE
                let p = unsafe { &mut *(data as *mut SndPcmHwParams) };
                if self.refine(p) {
                    Ok(0)
                } else {
                    Err(FsError::InvalidParam)
                }
            }
            0x11 => {
                // HW_PARAMS
                let p = unsafe { &mut *(data as *mut SndPcmHwParams) };
                self.install(p)?;
                Ok(0)
            }
            0x12 => {
                // HW_FREE
                let _ = self.audio.reset();
                self.st.lock().state = STATE_OPEN;
                Ok(0)
            }
            0x13 => {
                // SW_PARAMS
                let p = unsafe { &mut *(data as *mut SndPcmSwParams) };
                let mut st = self.st.lock();
                if p.avail_min > 0 {
                    st.avail_min = p.avail_min;
                }
                if p.boundary > 0 {
                    st.boundary = p.boundary;
                }
                Ok(0)
            }
            0x20 | 0x24 => {
                // STATUS / STATUS_EXT
                let s = unsafe { &mut *(data as *mut SndPcmStatus) };
                self.fill_status(s);
                Ok(0)
            }
            0x21 => {
                // DELAY
                unsafe { *(data as *mut i64) = self.queued_frames() as i64 };
                Ok(0)
            }
            0x22 => Ok(0), // HWSYNC — queued_bytes() reads live hardware state
            0x23 => {
                // SYNC_PTR — Linux flag semantics.
                let sp = unsafe { &mut *(data as *mut SndPcmSyncPtr) };
                let mut st = self.st.lock();
                if sp.flags & SYNC_PTR_APPL != 0 {
                    st.appl_ptr = sp.control.appl_ptr % st.boundary.max(1);
                } else {
                    sp.control.appl_ptr = st.appl_ptr;
                }
                if sp.flags & SYNC_PTR_AVAIL_MIN != 0 {
                    if sp.control.avail_min > 0 {
                        st.avail_min = sp.control.avail_min;
                    }
                } else {
                    sp.control.avail_min = st.avail_min;
                }
                let _ = sp.flags & SYNC_PTR_HWSYNC; // hw state is always live
                sp.status.state = st.state;
                sp.status.hw_ptr = self.hw_ptr(&st);
                sp.status.suspended_state = st.state;
                let now = kernel_hal::timer::timer_now();
                sp.status.tstamp = Timespec {
                    sec: now.as_secs() as i64,
                    nsec: now.subsec_nanos() as i64,
                };
                Ok(0)
            }
            0x40 => {
                // PREPARE
                let _ = self.audio.reset();
                let mut st = self.st.lock();
                st.appl_ptr = 0;
                st.state = STATE_PREPARED;
                Ok(0)
            }
            0x41 => {
                // RESET
                let _ = self.audio.reset();
                let mut st = self.st.lock();
                st.appl_ptr = 0;
                Ok(0)
            }
            0x42 => {
                // START — the driver starts the stream on first data; just
                // reflect the state change.
                self.st.lock().state = STATE_RUNNING;
                Ok(0)
            }
            0x43 => {
                // DROP
                let _ = self.audio.reset();
                self.st.lock().state = STATE_SETUP;
                Ok(0)
            }
            0x44 => self.drain().map(|_| 0),
            0x47 => Ok(0),                       // RESUME
            0x48 => Ok(0),                       // XRUN
            0x50 => {
                // WRITEI_FRAMES
                let xfer = unsafe { &mut *(data as *mut SndXferI) };
                self.writei(xfer)?;
                Ok(0)
            }
            _ => {
                debug!("[snd] pcm ioctl 'A' nr={:#x} unsupported", nr);
                Err(FsError::NotSupported)
            }
        }
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(Metadata {
            dev: 1,
            inode: self.inode_id,
            size: 0,
            blk_size: 0,
            blocks: 0,
            atime: Timespec_ZERO,
            mtime: Timespec_ZERO,
            ctime: Timespec_ZERO,
            type_: FileType::CharDevice,
            mode: 0o666,
            nlinks: 1,
            uid: 0,
            gid: 0,
            // ALSA: major 116, PCM playback dev = card*32 + 16 + device.
            rdev: make_rdev(116, self.card * 32 + 16),
        })
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}

#[allow(non_upper_case_globals)]
const Timespec_ZERO: rcore_fs::vfs::Timespec = rcore_fs::vfs::Timespec { sec: 0, nsec: 0 };

// ── Control device node ─────────────────────────────────────────────────────

pub struct CtlDev {
    audio: Arc<dyn AudioScheme>,
    card: usize,
    inode_id: usize,
}

impl CtlDev {
    pub fn new(audio: Arc<dyn AudioScheme>, card: usize) -> Self {
        CtlDev {
            audio,
            card,
            inode_id: DevFS::new_inode_id(),
        }
    }
}

impl INode for CtlDev {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: false,
            write: false,
            error: false,
        })
    }

    fn io_control(&self, cmd: u32, data: usize) -> Result<usize> {
        let ty = (cmd >> 8) & 0xff;
        let nr = cmd & 0xff;
        if ty != b'U' as u32 {
            return Err(FsError::NotSupported);
        }
        match nr {
            0x00 => {
                unsafe { *(data as *mut i32) = SNDRV_CTL_VERSION };
                Ok(0)
            }
            0x01 => {
                // CARD_INFO
                let info = unsafe { &mut *(data as *mut SndCtlCardInfo) };
                unsafe {
                    core::ptr::write_bytes(info as *mut SndCtlCardInfo as *mut u8, 0, core::mem::size_of::<SndCtlCardInfo>())
                };
                info.card = self.card as i32;
                fill_cstr(&mut info.id, &alloc::format!("EclipseHDA{}", self.card));
                fill_cstr(&mut info.driver, "eclipse-hda");
                fill_cstr(&mut info.name, self.audio.name());
                fill_cstr(&mut info.longname, self.audio.name());
                fill_cstr(&mut info.mixername, "Eclipse HDA (no mixer)");
                fill_cstr(&mut info.components, "");
                Ok(0)
            }
            0x10 => {
                // ELEM_LIST: no mixer controls yet.
                // struct: u32 offset, space, used, count; ptr; reserved.
                unsafe {
                    *(data as *mut u32).add(2) = 0; // used
                    *(data as *mut u32).add(3) = 0; // count
                }
                Ok(0)
            }
            // ELEM_INFO / ELEM_READ / ELEM_WRITE: no elements exist.
            0x11 | 0x12 | 0x13 => Err(FsError::EntryNotFound),
            0x30 => {
                // PCM_NEXT_DEVICE: single PCM device (0).
                let v = unsafe { &mut *(data as *mut i32) };
                *v = if *v < 0 { 0 } else { -1 };
                Ok(0)
            }
            0x31 => {
                // PCM_INFO
                let info = unsafe { &mut *(data as *mut SndPcmInfo) };
                if info.device != 0 || info.stream != 0 {
                    return Err(FsError::EntryNotFound);
                }
                let device = info.device;
                let subdevice = info.subdevice;
                unsafe {
                    core::ptr::write_bytes(info as *mut SndPcmInfo as *mut u8, 0, core::mem::size_of::<SndPcmInfo>())
                };
                info.device = device;
                info.subdevice = subdevice;
                info.stream = 0;
                info.card = self.card as i32;
                fill_cstr(&mut info.id, "Eclipse HDA");
                fill_cstr(&mut info.name, self.audio.name());
                fill_cstr(&mut info.subname, "subdevice #0");
                info.subdevices_count = 1;
                info.subdevices_avail = 1;
                Ok(0)
            }
            0x32 => Ok(0), // PCM_PREFER_SUBDEVICE
            _ => {
                debug!("[snd] ctl ioctl 'U' nr={:#x} unsupported", nr);
                Err(FsError::NotSupported)
            }
        }
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(Metadata {
            dev: 1,
            inode: self.inode_id,
            size: 0,
            blk_size: 0,
            blocks: 0,
            atime: Timespec_ZERO,
            mtime: Timespec_ZERO,
            ctime: Timespec_ZERO,
            type_: FileType::CharDevice,
            mode: 0o666,
            nlinks: 1,
            uid: 0,
            gid: 0,
            // ALSA: major 116, control dev = card*32.
            rdev: make_rdev(116, self.card * 32),
        })
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}
