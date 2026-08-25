//! Intel High Definition Audio controller + codec driver.
//!
//! Covers every PCI class-0x0403 HDA controller in one driver:
//!
//! * the PCH's onboard controller (00:1f.3 on the X299 board) with an analog
//!   codec behind it,
//! * the HDA function of NVIDIA GPUs (`xx:00.1`), whose codec exposes one
//!   HDMI/DP pin+converter pair per physical connector, and
//! * QEMU's `-device intel-hda -device hda-output`, which is how this driver
//!   is exercised in emulation.
//!
//! Design notes:
//!
//! * **Polled, no interrupts.** Codec verbs go through the CORB/RIRB DMA rings
//!   with polled responses (the immediate-command registers are optional in
//!   the spec and absent on some controllers). Playback progress is read from
//!   the stream's LPIB register. This mirrors the NVMe driver's philosophy:
//!   fewer moving parts on real hardware we cannot debug interactively.
//! * **One output stream, cyclic ring.** A physically contiguous PCM ring is
//!   described by a Buffer Descriptor List once; the hardware loops it
//!   forever while RUN is set. [`AudioScheme::write`] copies into the ring
//!   ahead of the DMA position; consumed regions are re-zeroed behind it so
//!   an underrun plays silence instead of looping stale audio.
//! * **HDMI vs analog is a codec-graph decision.** After the widget walk the
//!   driver prefers a connected (presence-detect) HDMI/DP pin; analog
//!   line-out/speaker/HP pins are the fallback. Both paths share the same
//!   converter setup; the HDMI path additionally enables the digital
//!   converter, channel count and audio infoframe (DIP) verbs.
//!
//! On NVIDIA GPUs the HDMI audio packets only reach the display if the
//! display engine is scanning out with audio enabled for that head (the
//! GSP-RM display path owns that); the pin's ELD-valid bit is logged at
//! bring-up so a silent output can be told apart from a stream that never
//! started.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

use lock::Mutex;
use pci::{PCIDevice, BAR};

use crate::builder::IoMapper;
use crate::bus::pci_drivers::PciDriver;
use crate::nvme::nvme_queue::{timer_now_as_micros, Provider, ProviderImpl, PAGE_SIZE};
use crate::scheme::{AudioScheme, Scheme};
use crate::{Device, DeviceError, DeviceResult};

// ── Controller MMIO registers (offsets from BAR0) ───────────────────────────
const REG_GCAP: usize = 0x00; // u16: global capabilities
const REG_GCTL: usize = 0x08; // u32: global control (bit0 = CRST)
const REG_STATESTS: usize = 0x0e; // u16: codec presence bitmap after reset
const REG_INTCTL: usize = 0x20; // u32: interrupt control (kept 0 — polled)
const REG_CORBLBASE: usize = 0x40;
const REG_CORBUBASE: usize = 0x44;
const REG_CORBWP: usize = 0x48; // u16: write pointer (low byte)
const REG_CORBRP: usize = 0x4a; // u16: read pointer, bit15 = reset
const REG_CORBCTL: usize = 0x4c; // u8: bit1 = DMA run
const REG_CORBSIZE: usize = 0x4e; // u8
const REG_RIRBLBASE: usize = 0x50;
const REG_RIRBUBASE: usize = 0x54;
const REG_RIRBWP: usize = 0x58; // u16: write pointer, write bit15 to reset
const REG_RINTCNT: usize = 0x5a; // u16
const REG_RIRBCTL: usize = 0x5c; // u8: bit1 = DMA run
const REG_RIRBSIZE: usize = 0x5e; // u8
const REG_SD_BASE: usize = 0x80; // stream descriptors, 0x20 bytes each

// Stream descriptor register offsets (from the descriptor base).
const SD_CTL: usize = 0x00; // u32 (24-bit): bit0 SRST, bit1 RUN, [23:20] tag
const SD_LPIB: usize = 0x04; // u32: link position in cyclic buffer
const SD_CBL: usize = 0x08; // u32: cyclic buffer length
const SD_LVI: usize = 0x0c; // u16: last valid BDL index
const SD_FMT: usize = 0x12; // u16: stream format
const SD_BDPL: usize = 0x18; // u32: BDL base low
const SD_BDPU: usize = 0x1c; // u32: BDL base high

const GCTL_CRST: u32 = 1 << 0;

// ── Codec verbs ─────────────────────────────────────────────────────────────
const VERB_GET_PARAMETER: u32 = 0xf00;
const VERB_GET_CONN_LIST: u32 = 0xf02;
const VERB_SET_CONN_SELECT: u32 = 0x701;
const VERB_SET_POWER_STATE: u32 = 0x705;
const VERB_SET_STREAM_ID: u32 = 0x706;
const VERB_SET_PIN_CTL: u32 = 0x707;
const VERB_GET_PIN_SENSE: u32 = 0xf09;
const VERB_SET_EAPD: u32 = 0x70c;
const VERB_SET_DIGI_CVT1: u32 = 0x70d;
const VERB_GET_CONFIG_DEFAULT: u32 = 0xf1c;
const VERB_SET_DIP_INDEX: u32 = 0x730;
const VERB_SET_DIP_DATA: u32 = 0x731;
const VERB_SET_DIP_XMIT: u32 = 0x732;
const VERB_SET_CVT_CHAN_COUNT: u32 = 0x733;

// GET_PARAMETER ids.
const PAR_VENDOR_ID: u32 = 0x00;
const PAR_NODE_COUNT: u32 = 0x04;
const PAR_FUNCTION_TYPE: u32 = 0x05;
const PAR_AUDIO_WIDGET_CAP: u32 = 0x09;
const PAR_PIN_CAP: u32 = 0x0c;
const PAR_CONN_LIST_LEN: u32 = 0x0e;
const PAR_OUT_AMP_CAP: u32 = 0x12;

// Widget types (audio widget caps [23:20]).
const WIDGET_AUDIO_OUT: u32 = 0x0;
const WIDGET_PIN: u32 = 0x4;

const PIN_CTL_OUT_EN: u32 = 0x40;

/// Time to wait for one codec verb response.
const VERB_TIMEOUT_US: u64 = 200_000;

/// PCM ring: 32 pages = 128 KiB (683 ms of 48 kHz S16LE stereo).
const RING_PAGES: usize = 32;
/// The BDL splits the ring into fixed 16 KiB cyclic segments.
const BDL_SEGMENT: usize = 16 * 1024;
/// Keep the software write pointer at least this far behind the DMA position
/// (the engine prefetches past LPIB into its FIFO).
const RING_GUARD: usize = 512;

// ── MMIO helpers ────────────────────────────────────────────────────────────
fn mmio_r8(bar: usize, off: usize) -> u8 {
    unsafe { read_volatile((bar + off) as *const u8) }
}
fn mmio_r16(bar: usize, off: usize) -> u16 {
    unsafe { read_volatile((bar + off) as *const u16) }
}
fn mmio_r32(bar: usize, off: usize) -> u32 {
    unsafe { read_volatile((bar + off) as *const u32) }
}
fn mmio_w8(bar: usize, off: usize, v: u8) {
    unsafe { write_volatile((bar + off) as *mut u8, v) }
}
fn mmio_w16(bar: usize, off: usize, v: u16) {
    unsafe { write_volatile((bar + off) as *mut u16, v) }
}
fn mmio_w32(bar: usize, off: usize, v: u32) {
    unsafe { write_volatile((bar + off) as *mut u32, v) }
}

fn clflush_range(vaddr: usize, len: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        use core::arch::x86_64::{_mm_clflush, _mm_mfence};
        let line = 64;
        let start = vaddr & !(line - 1);
        let end = (vaddr + len + line - 1) & !(line - 1);
        unsafe {
            _mm_mfence();
            for a in (start..end).step_by(line) {
                _mm_clflush(a as *const u8);
            }
            _mm_mfence();
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (vaddr, len);
}

fn wait_us(us: u64) {
    let start = timer_now_as_micros();
    while timer_now_as_micros().wrapping_sub(start) < us {
        core::hint::spin_loop();
    }
}

// ── Driver state ────────────────────────────────────────────────────────────
struct HdaInner {
    bar: usize,

    // CORB/RIRB rings.
    corb_va: usize,
    corb_entries: usize,
    rirb_va: usize,
    rirb_entries: usize,
    rirb_rp: usize,

    /// Codec address answering our verbs.
    cad: u32,

    // Chosen output path.
    conv_nid: u32,
    pin_nid: u32,
    digital: bool,
    /// Audio function group node (for re-routing).
    afg: u32,
    /// Every viable pin -> converter pair found by the widget walk. The
    /// choice among them is re-evaluated at stream start: on NVIDIA GPUs
    /// presence/ELD appear on the pins only once the display driver pushes
    /// the monitor's ELD (long after this driver's PCI probe).
    candidates: Vec<OutPath>,

    // Output stream descriptor.
    sd_base: usize,
    stream_tag: u32,

    // PCM ring.
    ring_va: usize,
    ring_len: usize,
    bdl_pa: usize,

    // Software state.
    running: bool,
    /// Next byte to write, offset into the ring.
    wp: usize,
    /// Bytes queued and not yet confirmed consumed.
    queued: usize,
    /// LPIB at the last progress poll.
    last_lpib: u32,
    /// Ring offset up to which consumed data has been re-zeroed.
    zero_ptr: usize,

    rate: u32,
    channels: u8,
}

pub struct HdaDevice {
    name: String,
    inner: Mutex<HdaInner>,
}

impl HdaInner {
    // ── Codec verb transport (CORB/RIRB, polled) ────────────────────────────
    fn corb_cmd(&mut self, verb: u32) -> DeviceResult<u32> {
        let bar = self.bar;
        let wp = (mmio_r16(bar, REG_CORBWP) as usize + 1) % self.corb_entries;
        unsafe { write_volatile((self.corb_va + wp * 4) as *mut u32, verb) };
        clflush_range(self.corb_va + wp * 4, 4);
        fence(Ordering::SeqCst);
        mmio_w16(bar, REG_CORBWP, wp as u16);

        let start = timer_now_as_micros();
        loop {
            let hw_wp = (mmio_r16(bar, REG_RIRBWP) & 0xff) as usize % self.rirb_entries;
            while self.rirb_rp != hw_wp {
                self.rirb_rp = (self.rirb_rp + 1) % self.rirb_entries;
                let entry_va = self.rirb_va + self.rirb_rp * 8;
                clflush_range(entry_va, 8);
                let resp = unsafe { read_volatile(entry_va as *const u32) };
                let ext = unsafe { read_volatile((entry_va + 4) as *const u32) };
                if ext & (1 << 4) != 0 {
                    // Unsolicited response (jack events) — not ours, keep going.
                    continue;
                }
                return Ok(resp);
            }
            if timer_now_as_micros().wrapping_sub(start) > VERB_TIMEOUT_US {
                warn!("[hda] verb {:#010x} timed out", verb);
                return Err(DeviceError::IoError);
            }
            core::hint::spin_loop();
        }
    }

    /// 12-bit verb with an 8-bit payload.
    fn cmd(&mut self, nid: u32, verb: u32, payload: u32) -> DeviceResult<u32> {
        self.corb_cmd((self.cad << 28) | (nid << 20) | (verb << 8) | (payload & 0xff))
    }

    /// 4-bit verb with a 16-bit payload (converter format / amp gain).
    fn cmd16(&mut self, nid: u32, verb: u32, payload: u32) -> DeviceResult<u32> {
        self.corb_cmd((self.cad << 28) | (nid << 20) | (verb << 16) | (payload & 0xffff))
    }

    fn param(&mut self, nid: u32, par: u32) -> DeviceResult<u32> {
        self.cmd(nid, VERB_GET_PARAMETER, par)
    }

    // ── Playback ring bookkeeping ───────────────────────────────────────────
    fn lpib(&self) -> u32 {
        mmio_r32(self.bar, self.sd_base + SD_LPIB)
    }

    /// Fold DMA progress since the last poll into `queued`, detect underrun,
    /// and re-zero consumed ring space (so an underrun loops silence).
    fn poll_progress(&mut self) {
        if !self.running {
            return;
        }
        let lpib = self.lpib();
        let consumed =
            (lpib as usize + self.ring_len - self.last_lpib as usize) % self.ring_len;
        self.last_lpib = lpib;
        if consumed >= self.queued && consumed > 0 {
            // The engine ran past everything we queued: underrun. Stop; the
            // next write restarts the stream. The ring behind the DMA position
            // is already zeroed, so whatever played out in the gap was silence.
            self.queued = 0;
            self.stop_stream();
            return;
        }
        self.queued -= consumed;

        // Re-zero what the DMA engine has consumed, staying RING_GUARD behind
        // its current position (it prefetches past LPIB).
        let safe_end = (lpib as usize + self.ring_len - RING_GUARD) % self.ring_len;
        let to_zero = (safe_end + self.ring_len - self.zero_ptr) % self.ring_len;
        // Only zero regions that are actually behind the queue tail.
        let behind = (self.wp + self.ring_len - self.zero_ptr) % self.ring_len;
        let n = to_zero.min(behind.saturating_sub(self.queued));
        let mut p = self.zero_ptr;
        let mut left = n;
        while left > 0 {
            let chunk = left.min(self.ring_len - p);
            unsafe { core::ptr::write_bytes((self.ring_va + p) as *mut u8, 0, chunk) };
            clflush_range(self.ring_va + p, chunk);
            p = (p + chunk) % self.ring_len;
            left -= chunk;
        }
        self.zero_ptr = p;
    }

    fn free_bytes(&self) -> usize {
        self.ring_len - RING_GUARD - self.queued
    }

    fn stop_stream(&mut self) {
        let ctl = mmio_r32(self.bar, self.sd_base + SD_CTL);
        mmio_w32(self.bar, self.sd_base + SD_CTL, ctl & !0x2);
        self.running = false;
    }

    /// Reset + program the stream descriptor and start the DMA engine.
    fn start_stream(&mut self) -> DeviceResult {
        let bar = self.bar;
        let sd = self.sd_base;

        // Stream reset handshake.
        mmio_w32(bar, sd + SD_CTL, 0x1);
        let t = timer_now_as_micros();
        while mmio_r32(bar, sd + SD_CTL) & 0x1 == 0 {
            if timer_now_as_micros().wrapping_sub(t) > 100_000 {
                break;
            }
            core::hint::spin_loop();
        }
        mmio_w32(bar, sd + SD_CTL, 0x0);
        let t = timer_now_as_micros();
        while mmio_r32(bar, sd + SD_CTL) & 0x1 != 0 {
            if timer_now_as_micros().wrapping_sub(t) > 100_000 {
                break;
            }
            core::hint::spin_loop();
        }

        let fmt = stream_format(self.rate, self.channels);
        mmio_w32(bar, sd + SD_BDPL, self.bdl_pa as u32);
        mmio_w32(bar, sd + SD_BDPU, (self.bdl_pa as u64 >> 32) as u32);
        mmio_w32(bar, sd + SD_CBL, self.ring_len as u32);
        mmio_w16(bar, sd + SD_LVI, (self.ring_len / BDL_SEGMENT - 1) as u16);
        mmio_w16(bar, sd + SD_FMT, fmt);

        // The converter must agree with the descriptor on format and stream.
        let conv = self.conv_nid;
        self.cmd16(conv, 0x2, fmt as u32)?;
        self.cmd(conv, VERB_SET_STREAM_ID, self.stream_tag << 4)?;

        fence(Ordering::SeqCst);
        // Tag + RUN.
        mmio_w32(bar, sd + SD_CTL, (self.stream_tag << 20) | 0x2);

        self.last_lpib = 0;
        self.running = true;
        Ok(())
    }
}

/// Encode an HDA stream format: S16LE, `channels` interleaved, `rate` Hz.
/// Rates outside the encodable set were normalized by `nearest_rate`.
fn stream_format(rate: u32, channels: u8) -> u16 {
    // (base44, mult, div)
    let (base44, mult, div) = match rate {
        8000 => (false, 1, 6),
        11025 => (true, 1, 4),
        16000 => (false, 1, 3),
        22050 => (true, 1, 2),
        32000 => (false, 2, 3),
        44100 => (true, 1, 1),
        88200 => (true, 2, 1),
        96000 => (false, 2, 1),
        176400 => (true, 4, 1),
        192000 => (false, 4, 1),
        _ => (false, 1, 1), // 48000
    };
    let mut fmt: u16 = 0;
    if base44 {
        fmt |= 1 << 14;
    }
    fmt |= ((mult - 1) as u16) << 11;
    fmt |= ((div - 1) as u16) << 8;
    fmt |= 0b001 << 4; // 16-bit
    fmt |= (channels as u16 - 1) & 0xf;
    fmt
}

fn nearest_rate(rate: u32) -> u32 {
    const RATES: [u32; 11] = [
        8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000,
    ];
    *RATES
        .iter()
        .min_by_key(|&&r| (r as i64 - rate as i64).unsigned_abs())
        .unwrap_or(&48000)
}

// ── Codec graph walk ────────────────────────────────────────────────────────
#[derive(Clone)]
struct OutPath {
    conv: u32,
    pin: u32,
    pin_conn_idx: u32,
    digital: bool,
    hdmi_dp: bool,
    present: bool,
}

fn sub_nodes(resp: u32) -> (u32, u32) {
    ((resp >> 16) & 0xff, resp & 0xff)
}

impl HdaInner {
    /// Read a pin's connection list and return the index of `conv` in it.
    fn conn_index_of(&mut self, pin: u32, conv: u32) -> DeviceResult<Option<u32>> {
        let len_resp = self.param(pin, PAR_CONN_LIST_LEN)?;
        let long_form = len_resp & 0x80 != 0;
        let len = len_resp & 0x7f;
        let per_resp = if long_form { 2 } else { 4 };
        let mut idx = 0u32;
        let mut i = 0u32;
        while i < len {
            let resp = self.cmd(pin, VERB_GET_CONN_LIST, i)?;
            for k in 0..per_resp.min(len - i) {
                let entry = if long_form {
                    (resp >> (16 * k)) & 0x7fff
                } else {
                    (resp >> (8 * k)) & 0x7f
                };
                if entry == conv {
                    return Ok(Some(idx));
                }
                idx += 1;
            }
            i += per_resp;
        }
        Ok(None)
    }

    /// Fresh score for a candidate path: prefer digital HDMI/DP pins, then
    /// jack/display presence, then a valid ELD (on NVIDIA GPUs presence and
    /// ELD appear only after the display driver pushes the monitor's ELD).
    fn score_path(&mut self, p: &OutPath) -> (i32, bool) {
        let sense = self.cmd(p.pin, VERB_GET_PIN_SENSE, 0).unwrap_or(0);
        let present = sense & (1 << 31) != 0;
        let eld_valid = sense & (1 << 30) != 0;
        let mut score = 0;
        if p.hdmi_dp && p.digital {
            score += 4;
        }
        if present {
            score += 2;
        }
        if eld_valid {
            score += 1;
        }
        (score, present)
    }

    /// Enumerate the AFG's widgets and collect every viable output path
    /// (output-capable pin with a physical connector, reachable converter).
    fn collect_candidates(&mut self, afg: u32) -> DeviceResult<Vec<OutPath>> {
        let (wstart, wcount) = sub_nodes(self.param(afg, PAR_NODE_COUNT)?);
        let mut converters: Vec<(u32, bool)> = Vec::new(); // (nid, digital)
        let mut pins: Vec<(u32, u32)> = Vec::new(); // (nid, pincap)

        for nid in wstart..wstart + wcount {
            let caps = self.param(nid, PAR_AUDIO_WIDGET_CAP)?;
            let wtype = (caps >> 20) & 0xf;
            let digital = caps & (1 << 9) != 0;
            match wtype {
                WIDGET_AUDIO_OUT => converters.push((nid, digital)),
                WIDGET_PIN => {
                    let pincap = self.param(nid, PAR_PIN_CAP)?;
                    let defcfg = self.cmd(nid, VERB_GET_CONFIG_DEFAULT, 0)?;
                    // Output-capable pins with a physical connection only.
                    let connectivity = (defcfg >> 30) & 0x3;
                    if pincap & (1 << 4) != 0 && connectivity != 0x1 {
                        pins.push((nid, pincap));
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for &(pin, pincap) in &pins {
            let hdmi_dp = pincap & (1 << 7) != 0 || pincap & (1 << 24) != 0;
            // First reachable converter per pin is enough.
            for &(conv, cdigital) in &converters {
                if let Some(idx) = self.conn_index_of(pin, conv)? {
                    out.push(OutPath {
                        conv,
                        pin,
                        pin_conn_idx: idx,
                        digital: cdigital,
                        hdmi_dp,
                        present: false,
                    });
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Pick the best-scoring path among `self.candidates` right now.
    fn best_candidate(&mut self) -> Option<OutPath> {
        let candidates = self.candidates.clone();
        let mut best: Option<OutPath> = None;
        let mut best_score = -1i32;
        for mut p in candidates {
            let (score, present) = self.score_path(&p);
            p.present = present;
            info!(
                "[hda] path candidate: pin {:#x} -> conv {:#x} (digital={}, hdmi/dp={}, present={}, score={})",
                p.pin, p.conv, p.digital, p.hdmi_dp, present, score
            );
            if score > best_score {
                best_score = score;
                best = Some(p);
            }
        }
        best
    }

    /// Re-evaluate the candidate paths and re-route if a better pin has
    /// appeared (e.g. the display driver pushed the monitor's ELD after our
    /// PCI-probe-time pick). Called with the stream stopped.
    fn repick_path(&mut self) {
        if self.candidates.len() < 2 {
            return;
        }
        let afg = self.afg;
        if let Some(best) = self.best_candidate() {
            if best.pin != self.pin_nid {
                info!(
                    "[hda] re-routing output: pin {:#x} -> pin {:#x}",
                    self.pin_nid, best.pin
                );
                if let Err(e) = self.setup_path(afg, &best) {
                    warn!("[hda] re-route failed: {:?} — keeping previous path", e);
                }
            }
        }
    }

    /// Power up and route the chosen path.
    fn setup_path(&mut self, afg: u32, path: &OutPath) -> DeviceResult {
        self.conv_nid = path.conv;
        self.pin_nid = path.pin;
        self.digital = path.digital;

        self.cmd(afg, VERB_SET_POWER_STATE, 0)?; // AFG -> D0
        wait_us(1_000);
        self.cmd(path.conv, VERB_SET_POWER_STATE, 0)?;
        self.cmd(path.pin, VERB_SET_POWER_STATE, 0)?;

        self.cmd(path.pin, VERB_SET_CONN_SELECT, path.pin_conn_idx)?;
        self.cmd(path.pin, VERB_SET_PIN_CTL, PIN_CTL_OUT_EN)?;

        // EAPD (external amplifier) on pins that have it — analog outputs.
        let pincap = self.param(path.pin, PAR_PIN_CAP)?;
        if pincap & (1 << 16) != 0 {
            let _ = self.cmd(path.pin, VERB_SET_EAPD, 0x2);
        }

        // Unmute + 0 dB on any output amps in the path.
        for &nid in &[path.conv, path.pin] {
            let amp = self.param(nid, PAR_OUT_AMP_CAP)?;
            let has_amp = amp != 0
                || self.param(nid, PAR_AUDIO_WIDGET_CAP)? & (1 << 2) != 0;
            if has_amp {
                let offset = amp & 0x7f; // 0 dB step index
                // Set output amp, both channels, gain = offset (0 dB).
                let payload = (1 << 15) | (1 << 13) | (1 << 12) | offset;
                let _ = self.cmd16(nid, 0x3, payload);
            }
        }

        if path.digital {
            // Enable the digital converter and declare 2 channels.
            self.cmd(path.conv, VERB_SET_DIGI_CVT1, 0x1)?;
            let _ = self.cmd(path.conv, VERB_SET_CVT_CHAN_COUNT, 1);
            self.send_audio_infoframe(2);
        }
        Ok(())
    }

    /// Program the pin's Data Island Packet buffer with a CEA audio infoframe
    /// (HDMI sinks want one before they render PCM). Best-effort: some codecs
    /// route infoframes through the graphics driver instead.
    fn send_audio_infoframe(&mut self, channels: u8) {
        let pin = self.pin_nid;
        let mut frame = [0u8; 14];
        frame[0] = 0x84; // CEA audio infoframe
        frame[1] = 0x01; // version
        frame[2] = 0x0a; // payload length
        frame[4] = channels - 1; // CC, coding type "refer to stream"
        // frame[8] = CA (0 = FL/FR), rest zero.
        let sum: u32 = frame.iter().map(|&b| b as u32).sum();
        frame[3] = (0x100 - (sum & 0xff) as u16 as u32 & 0xff) as u8;

        let _ = self.cmd(pin, VERB_SET_DIP_INDEX, 0);
        for &b in &frame {
            let _ = self.cmd(pin, VERB_SET_DIP_DATA, b as u32);
        }
        let _ = self.cmd(pin, VERB_SET_DIP_XMIT, 0xc0); // best-effort transmit
    }
}

impl HdaDevice {
    pub fn new(bar: usize, name: String) -> DeviceResult<Self> {
        let gcap = mmio_r16(bar, REG_GCAP);
        let iss = ((gcap >> 8) & 0xf) as usize;
        let oss = ((gcap >> 12) & 0xf) as usize;
        let addr64 = gcap & 1 != 0;
        info!(
            "[hda] {}: GCAP {:#06x} — {} in / {} out streams, 64-bit {}",
            name, gcap, iss, oss, addr64
        );
        if oss == 0 {
            return Err(DeviceError::NotSupported);
        }

        // ── Controller reset ────────────────────────────────────────────────
        mmio_w32(bar, REG_GCTL, mmio_r32(bar, REG_GCTL) & !GCTL_CRST);
        let t = timer_now_as_micros();
        while mmio_r32(bar, REG_GCTL) & GCTL_CRST != 0 {
            if timer_now_as_micros().wrapping_sub(t) > 1_000_000 {
                warn!("[hda] {}: controller reset entry timed out", name);
                return Err(DeviceError::IoError);
            }
            core::hint::spin_loop();
        }
        wait_us(200);
        mmio_w32(bar, REG_GCTL, mmio_r32(bar, REG_GCTL) | GCTL_CRST);
        let t = timer_now_as_micros();
        while mmio_r32(bar, REG_GCTL) & GCTL_CRST == 0 {
            if timer_now_as_micros().wrapping_sub(t) > 1_000_000 {
                warn!("[hda] {}: controller reset exit timed out", name);
                return Err(DeviceError::IoError);
            }
            core::hint::spin_loop();
        }
        // Codecs get 521 µs (25 frames) to request state-change; be generous.
        wait_us(2_000);

        let statests = mmio_r16(bar, REG_STATESTS);
        if statests == 0 {
            warn!("[hda] {}: no codec responded after reset", name);
            return Err(DeviceError::NoResources);
        }
        let cad = statests.trailing_zeros();

        // Polled operation: all interrupt sources off.
        mmio_w32(bar, REG_INTCTL, 0);

        // ── CORB/RIRB ───────────────────────────────────────────────────────
        mmio_w8(bar, REG_CORBCTL, 0);
        mmio_w8(bar, REG_RIRBCTL, 0);

        // Pick the largest supported ring: SIZECAP bits [7:4] = {2,16,256}.
        let corb_szcap = mmio_r8(bar, REG_CORBSIZE) >> 4;
        let (corb_entries, corb_sz) = ring_size(corb_szcap);
        mmio_w8(bar, REG_CORBSIZE, corb_sz);
        let rirb_szcap = mmio_r8(bar, REG_RIRBSIZE) >> 4;
        let (rirb_entries, rirb_sz) = ring_size(rirb_szcap);
        mmio_w8(bar, REG_RIRBSIZE, rirb_sz);

        let (corb_va, corb_pa) = ProviderImpl::alloc_dma(PAGE_SIZE);
        let (rirb_va, rirb_pa) = ProviderImpl::alloc_dma(PAGE_SIZE);
        unsafe {
            core::ptr::write_bytes(corb_va as *mut u8, 0, PAGE_SIZE);
            core::ptr::write_bytes(rirb_va as *mut u8, 0, PAGE_SIZE);
        }
        clflush_range(corb_va, PAGE_SIZE);
        clflush_range(rirb_va, PAGE_SIZE);

        mmio_w32(bar, REG_CORBLBASE, corb_pa as u32);
        mmio_w32(bar, REG_CORBUBASE, (corb_pa as u64 >> 32) as u32);
        mmio_w32(bar, REG_RIRBLBASE, rirb_pa as u32);
        mmio_w32(bar, REG_RIRBUBASE, (rirb_pa as u64 >> 32) as u32);

        // CORB read-pointer reset handshake (best-effort: QEMU acks lazily).
        mmio_w16(bar, REG_CORBRP, 1 << 15);
        wait_us(100);
        mmio_w16(bar, REG_CORBRP, 0);
        wait_us(100);
        mmio_w16(bar, REG_CORBWP, 0);
        // RIRB write-pointer reset (self-clearing).
        mmio_w16(bar, REG_RIRBWP, 1 << 15);
        mmio_w16(bar, REG_RINTCNT, 1);

        mmio_w8(bar, REG_RIRBCTL, 0x2); // RIRB DMA run
        mmio_w8(bar, REG_CORBCTL, 0x2); // CORB DMA run

        // ── PCM ring + BDL ─────────────────────────────────────────────────
        let ring_len = RING_PAGES * PAGE_SIZE;
        let (ring_va, ring_pa) = ProviderImpl::alloc_dma(ring_len);
        let (bdl_va, bdl_pa) = ProviderImpl::alloc_dma(PAGE_SIZE);
        unsafe { core::ptr::write_bytes(ring_va as *mut u8, 0, ring_len) };
        clflush_range(ring_va, ring_len);
        let n_seg = ring_len / BDL_SEGMENT;
        for i in 0..n_seg {
            let e = bdl_va + i * 16;
            unsafe {
                write_volatile(e as *mut u64, (ring_pa + i * BDL_SEGMENT) as u64);
                write_volatile((e + 8) as *mut u32, BDL_SEGMENT as u32);
                write_volatile((e + 12) as *mut u32, 0); // no IOC — polled
            }
        }
        clflush_range(bdl_va, n_seg * 16);

        let mut inner = HdaInner {
            bar,
            corb_va,
            corb_entries,
            rirb_va,
            rirb_entries,
            rirb_rp: 0,
            cad,
            conv_nid: 0,
            pin_nid: 0,
            digital: false,
            afg: 0,
            candidates: Vec::new(),
            // First output stream descriptor comes after the input ones.
            sd_base: REG_SD_BASE + iss * 0x20,
            stream_tag: 1,
            ring_va,
            ring_len,
            bdl_pa,
            running: false,
            wp: 0,
            queued: 0,
            last_lpib: 0,
            zero_ptr: 0,
            rate: 48000,
            channels: 2,
        };

        // ── Codec discovery ────────────────────────────────────────────────
        let vendor = inner.param(0, PAR_VENDOR_ID)?;
        info!("[hda] {}: codec {} vendor {:#010x}", name, cad, vendor);

        let (fg_start, fg_count) = sub_nodes(inner.param(0, PAR_NODE_COUNT)?);
        let mut afg = None;
        for nid in fg_start..fg_start + fg_count {
            if inner.param(nid, PAR_FUNCTION_TYPE)? & 0xff == 0x01 {
                afg = Some(nid);
                break;
            }
        }
        let afg = afg.ok_or(DeviceError::NotSupported)?;

        inner.afg = afg;
        inner.candidates = inner.collect_candidates(afg)?;
        let path = inner.best_candidate().ok_or(DeviceError::NotSupported)?;
        info!(
            "[hda] {}: using pin {:#x} -> converter {:#x} ({}, {})",
            name,
            path.pin,
            path.conv,
            if path.digital { "HDMI/DP" } else { "analog" },
            if path.present {
                "display/jack present"
            } else {
                "nothing detected on jack — playing anyway"
            }
        );
        inner.setup_path(afg, &path)?;

        Ok(HdaDevice {
            name,
            inner: Mutex::new(inner),
        })
    }
}

/// Map a CORB/RIRB SIZECAP field to the largest supported (entries, SIZE code).
fn ring_size(szcap: u8) -> (usize, u8) {
    if szcap & 0x4 != 0 || szcap == 0 {
        // 256 entries (assume 256 when the cap field reads zero — fixed-size
        // controllers may hardwire it).
        (256, 0x2)
    } else if szcap & 0x2 != 0 {
        (16, 0x1)
    } else {
        (2, 0x0)
    }
}

impl Scheme for HdaDevice {
    fn name(&self) -> &str {
        &self.name
    }

    fn handle_irq(&self, _irq: usize) {
        // Fully polled.
    }
}

impl AudioScheme for HdaDevice {
    fn set_params(&self, rate: u32, channels: u8) -> DeviceResult<(u32, u8)> {
        let rate = nearest_rate(rate);
        let _ = channels;
        let channels = 2u8; // stereo only for now
        let mut inner = self.inner.lock();
        inner.stop_stream();
        inner.queued = 0;
        inner.wp = 0;
        inner.zero_ptr = 0;
        unsafe { core::ptr::write_bytes(inner.ring_va as *mut u8, 0, inner.ring_len) };
        clflush_range(inner.ring_va, inner.ring_len);
        inner.rate = rate;
        inner.channels = channels;
        if inner.digital {
            let ch = channels;
            inner.send_audio_infoframe(ch);
        }
        Ok((rate, channels))
    }

    fn params(&self) -> (u32, u8) {
        let inner = self.inner.lock();
        (inner.rate, inner.channels)
    }

    fn write(&self, pcm: &[u8]) -> DeviceResult<usize> {
        let mut inner = self.inner.lock();
        inner.poll_progress();
        if !inner.running {
            // Stream (re)start: give the codec graph a chance to re-route to a
            // pin that has gained presence/ELD since the last pick (on NVIDIA
            // GPUs the display driver pushes the ELD long after PCI probe)...
            inner.repick_path();
            // ...and re-anchor the software pointers: a started stream always
            // begins DMA at ring offset 0.
            inner.wp = 0;
            inner.zero_ptr = 0;
            inner.queued = 0;
        }
        let free = inner.free_bytes();
        // Whole frames only, so channels never swap on a partial write.
        let frame = inner.channels as usize * 2;
        let n = free.min(pcm.len()) / frame * frame;
        if n == 0 {
            return Ok(0);
        }
        let mut p = inner.wp;
        let mut done = 0;
        while done < n {
            let chunk = (n - done).min(inner.ring_len - p);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    pcm.as_ptr().add(done),
                    (inner.ring_va + p) as *mut u8,
                    chunk,
                );
            }
            clflush_range(inner.ring_va + p, chunk);
            p = (p + chunk) % inner.ring_len;
            done += chunk;
        }
        inner.wp = p;
        inner.queued += n;
        if !inner.running {
            inner.start_stream()?;
        }
        Ok(n)
    }

    fn free_bytes(&self) -> usize {
        let mut inner = self.inner.lock();
        inner.poll_progress();
        inner.free_bytes()
    }

    fn buffer_bytes(&self) -> usize {
        let inner = self.inner.lock();
        inner.ring_len - RING_GUARD
    }

    fn queued_bytes(&self) -> usize {
        let mut inner = self.inner.lock();
        inner.poll_progress();
        inner.queued
    }

    fn reset(&self) -> DeviceResult {
        let mut inner = self.inner.lock();
        inner.stop_stream();
        inner.queued = 0;
        inner.wp = 0;
        inner.zero_ptr = 0;
        unsafe { core::ptr::write_bytes(inner.ring_va as *mut u8, 0, inner.ring_len) };
        clflush_range(inner.ring_va, inner.ring_len);
        Ok(())
    }
}

// ── PCI driver registration ─────────────────────────────────────────────────

pub struct HdaDriverPci;

impl PciDriver for HdaDriverPci {
    fn name(&self) -> &str {
        "hda-audio"
    }

    fn matched(&self, _vendor_id: u16, _device_id: u16) -> bool {
        false
    }

    fn matched_dev(&self, dev: &PCIDevice) -> bool {
        // PCI class 04 (multimedia), subclass 03 (HD Audio).
        dev.id.class == 0x04 && dev.id.subclass == 0x03
    }

    fn init(
        &self,
        dev: &PCIDevice,
        mapper: &Option<Arc<dyn IoMapper>>,
        _irq: Option<usize>,
    ) -> DeviceResult<Device> {
        let addr = match dev.bars[0] {
            Some(BAR::Memory(addr, _len, _, _)) => addr,
            _ => return Err(DeviceError::NotSupported),
        };
        if let Some(m) = mapper {
            m.query_or_map(addr as usize, 0x4000);
        }

        // NVIDIA HDA functions default to non-snooped DMA; force the coherent
        // path (same PCI config bytes the Linux azx driver programs) so our
        // cached CORB/RIRB/BDL/PCM buffers work without uncached mappings.
        if dev.id.vendor_id == 0x10de {
            let ops = &crate::bus::pci::PortOpsImpl;
            let am = crate::bus::pci::PCI_ACCESS;
            unsafe {
                let v = am.read8(ops, dev.loc, 0x4e);
                am.write8(ops, dev.loc, 0x4e, (v & 0xf0) | 0x0f);
                let v = am.read8(ops, dev.loc, 0x4c);
                am.write8(ops, dev.loc, 0x4c, v | 0x01);
                let v = am.read8(ops, dev.loc, 0x4d);
                am.write8(ops, dev.loc, 0x4d, v | 0x01);
            }
        }

        let vaddr = crate::bus::phys_to_virt(addr as usize);
        let name = alloc::format!(
            "hda-{:02x}:{:02x}.{:x}{}",
            dev.loc.bus,
            dev.loc.device,
            dev.loc.function,
            if dev.id.vendor_id == 0x10de {
                " (nvidia-hdmi)"
            } else {
                ""
            }
        );
        let hda = Arc::new(HdaDevice::new(vaddr, name)?);
        Ok(Device::Audio(hda))
    }
}
