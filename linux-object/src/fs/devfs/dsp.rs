//! `/dev/dsp` — OSS-style PCM playback node over an [`AudioScheme`] device.
//!
//! The OSS interface is the smallest kernel audio ABI that stock userspace
//! can drive: `write(2)` carries interleaved S16LE PCM, a handful of ioctls
//! negotiate rate/format/channels (`mpg123 -o oss`, `sox -t oss`, ffmpeg's
//! `-f oss` all speak it, and a plain `cat music.raw > /dev/dsp` works for
//! 48 kHz stereo, the default). One node is created per HDA device found:
//! `dsp` (first controller, usually the PCH's analog codec), `dsp1`, `dsp2`…
//! (the NVIDIA HDMI audio functions on this machine's RTX cards).
//!
//! Writes block by spin-retrying against the device ring, the same pattern
//! the TCP send path uses for a full TX buffer (`FileLike::write` is a
//! synchronous trait, so parking on a waker is not available here). The ring
//! drains at the PCM byte rate, so each retry loop is short-lived.

use alloc::sync::Arc;
use core::any::Any;

use kernel_hal::drivers::scheme::AudioScheme;
use rcore_fs::vfs::*;
use rcore_fs_devfs::DevFS;

// OSS ioctl numbers (Linux _IOC encoding of <sys/soundcard.h>).
const SNDCTL_DSP_RESET: u32 = 0x0000_5000; // _IO('P', 0)
const SNDCTL_DSP_SYNC: u32 = 0x0000_5001; // _IO('P', 1)
const SNDCTL_DSP_SPEED: u32 = 0xc004_5002; // _IOWR('P', 2, int)
const SNDCTL_DSP_STEREO: u32 = 0xc004_5003; // _IOWR('P', 3, int)
const SNDCTL_DSP_GETBLKSIZE: u32 = 0xc004_5004; // _IOWR('P', 4, int)
const SNDCTL_DSP_SETFMT: u32 = 0xc004_5005; // _IOWR('P', 5, int)
const SNDCTL_DSP_CHANNELS: u32 = 0xc004_5006; // _IOWR('P', 6, int)
const SNDCTL_DSP_POST: u32 = 0x0000_5008; // _IO('P', 8)
const SNDCTL_DSP_SETFRAGMENT: u32 = 0xc004_500a; // _IOWR('P', 10, int)
const SNDCTL_DSP_GETFMTS: u32 = 0x8004_500b; // _IOR('P', 11, int)
const SNDCTL_DSP_GETOSPACE: u32 = 0x8010_500c; // _IOR('P', 12, audio_buf_info)

const AFMT_S16_LE: i32 = 0x10;

/// `audio_buf_info` for GETOSPACE.
#[repr(C)]
struct AudioBufInfo {
    fragments: i32,
    fragstotal: i32,
    fragsize: i32,
    bytes: i32,
}

/// Nominal fragment size reported to OSS clients. The driver rings are byte
/// oriented; fragments only shape client buffering decisions.
const FRAG_SIZE: usize = 4096;

pub struct DspDev {
    audio: Arc<dyn AudioScheme>,
    index: usize,
    inode_id: usize,
}

impl DspDev {
    pub fn new(audio: Arc<dyn AudioScheme>, index: usize) -> Self {
        DspDev {
            audio,
            index,
            inode_id: DevFS::new_inode_id(),
        }
    }

    /// Seconds it takes the device to drain `bytes` at the current format,
    /// rounded up, plus one — used to bound waits without cutting audio off.
    fn drain_secs(&self, bytes: usize) -> u64 {
        let (rate, channels) = self.audio.params();
        let bps = (rate as u64) * (channels as u64) * 2;
        (bytes as u64).div_ceil(bps.max(1)) + 1
    }
}

impl INode for DspDev {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        // Playback only — no capture path yet.
        Err(FsError::NotSupported)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        let mut done = 0usize;
        let deadline_step =
            core::time::Duration::from_secs(self.drain_secs(self.audio.buffer_bytes()));
        let mut deadline = kernel_hal::timer::timer_now() + deadline_step;
        while done < buf.len() {
            let n = self.audio.write(&buf[done..]).map_err(|_| FsError::DeviceError)?;
            if n > 0 {
                done += n;
                deadline = kernel_hal::timer::timer_now() + deadline_step;
                continue;
            }
            // Ring full: the device frees space at the PCM byte rate. The
            // synchronous INode contract leaves no waker to park on, so
            // spin-retry like the TCP send path does, bounded so a wedged
            // stream cannot hang the writer forever.
            if kernel_hal::timer::timer_now() >= deadline {
                warn!("[dsp{}] playback ring made no progress; giving up", self.index);
                return if done > 0 { Ok(done) } else { Err(FsError::DeviceError) };
            }
            kernel_hal::deferred_job::drain_deferred_jobs();
            core::hint::spin_loop();
        }
        Ok(done)
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: false,
            write: self.audio.free_bytes() > 0,
            error: false,
        })
    }

    #[allow(unsafe_code)]
    fn io_control(&self, cmd: u32, data: usize) -> Result<usize> {
        match cmd {
            SNDCTL_DSP_RESET => {
                self.audio.reset().map_err(|_| FsError::DeviceError)?;
                Ok(0)
            }
            SNDCTL_DSP_SYNC | SNDCTL_DSP_POST => {
                // Drain: wait until everything queued has played out.
                let deadline = kernel_hal::timer::timer_now()
                    + core::time::Duration::from_secs(
                        self.drain_secs(self.audio.queued_bytes()),
                    );
                while self.audio.queued_bytes() > 0 {
                    if kernel_hal::timer::timer_now() >= deadline {
                        break;
                    }
                    kernel_hal::deferred_job::drain_deferred_jobs();
                    core::hint::spin_loop();
                }
                Ok(0)
            }
            SNDCTL_DSP_SPEED => {
                let val = unsafe { &mut *(data as *mut i32) };
                let (rate, _) = self
                    .audio
                    .set_params((*val).max(0) as u32, 2)
                    .map_err(|_| FsError::DeviceError)?;
                *val = rate as i32;
                Ok(0)
            }
            SNDCTL_DSP_SETFMT => {
                let val = unsafe { &mut *(data as *mut i32) };
                // S16LE is the only format; report it back whatever was asked.
                *val = AFMT_S16_LE;
                Ok(0)
            }
            SNDCTL_DSP_GETFMTS => {
                let val = unsafe { &mut *(data as *mut i32) };
                *val = AFMT_S16_LE;
                Ok(0)
            }
            SNDCTL_DSP_CHANNELS => {
                let val = unsafe { &mut *(data as *mut i32) };
                let (_, channels) = self.audio.params();
                *val = channels as i32;
                Ok(0)
            }
            SNDCTL_DSP_STEREO => {
                let val = unsafe { &mut *(data as *mut i32) };
                *val = 1; // stereo
                Ok(0)
            }
            SNDCTL_DSP_GETBLKSIZE => {
                let val = unsafe { &mut *(data as *mut i32) };
                *val = FRAG_SIZE as i32;
                Ok(0)
            }
            SNDCTL_DSP_SETFRAGMENT => {
                // Accepted but the driver keeps its own ring geometry.
                Ok(0)
            }
            SNDCTL_DSP_GETOSPACE => {
                let info = unsafe { &mut *(data as *mut AudioBufInfo) };
                let free = self.audio.free_bytes();
                let total = self.audio.buffer_bytes();
                info.fragments = (free / FRAG_SIZE) as i32;
                info.fragstotal = (total / FRAG_SIZE) as i32;
                info.fragsize = FRAG_SIZE as i32;
                info.bytes = free as i32;
                Ok(0)
            }
            _ => {
                warn!("[dsp{}] unsupported ioctl {:#x}", self.index, cmd);
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
            atime: Timespec { sec: 0, nsec: 0 },
            mtime: Timespec { sec: 0, nsec: 0 },
            ctime: Timespec { sec: 0, nsec: 0 },
            type_: FileType::CharDevice,
            mode: 0o666,
            nlinks: 1,
            uid: 0,
            gid: 0,
            // OSS numbering: /dev/dsp is (14, 3), /dev/dsp1 is (14, 19), …
            rdev: make_rdev(14, 3 + self.index * 16),
        })
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}
