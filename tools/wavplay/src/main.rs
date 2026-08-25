//! wavplay — minimal OSS PCM player for Eclipse OS.
//!
//! Usage:
//!   wavplay [-d /dev/dspN] file.wav     play a PCM WAV file (S16LE / mono / stereo)
//!   wavplay [-d /dev/dspN] --tone [HZ]  play a 3-second sine test tone (default 440 Hz)
//!
//! The default device is /dev/dsp (first HDA controller). On a machine with
//! NVIDIA HDMI audio the GPU functions are /dev/dsp1, /dev/dsp2, …

use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::process::exit;

const SNDCTL_DSP_SPEED: libc::c_ulong = 0xc004_5002;
const SNDCTL_DSP_SETFMT: libc::c_ulong = 0xc004_5005;
const SNDCTL_DSP_CHANNELS: libc::c_ulong = 0xc004_5006;
const SNDCTL_DSP_SYNC: libc::c_ulong = 0x0000_5001;
const AFMT_S16_LE: i32 = 0x10;

fn ioctl_int(fd: i32, req: libc::c_ulong, mut val: i32, name: &str) -> i32 {
    let r = unsafe { libc::ioctl(fd, req as _, &mut val as *mut i32) };
    if r < 0 {
        eprintln!("wavplay: ioctl {name} failed");
        exit(1);
    }
    val
}

struct Wav {
    channels: u16,
    rate: u32,
    data: Vec<u8>,
}

fn parse_wav(mut f: File) -> Result<Wav, String> {
    let mut hdr = [0u8; 12];
    f.read_exact(&mut hdr).map_err(|e| e.to_string())?;
    if &hdr[0..4] != b"RIFF" || &hdr[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut channels = 0u16;
    let mut rate = 0u32;
    let mut bits = 0u16;
    let mut fmt_tag = 0u16;
    let mut data = Vec::new();
    loop {
        let mut ch = [0u8; 8];
        match f.read_exact(&mut ch) {
            Ok(()) => {}
            Err(_) => break,
        }
        let id = &ch[0..4];
        let len = u32::from_le_bytes(ch[4..8].try_into().unwrap()) as usize;
        match id {
            b"fmt " => {
                let mut fmt = vec![0u8; len];
                f.read_exact(&mut fmt).map_err(|e| e.to_string())?;
                fmt_tag = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
                channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
                rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
                bits = u16::from_le_bytes(fmt[14..16].try_into().unwrap());
            }
            b"data" => {
                data = vec![0u8; len];
                f.read_exact(&mut data).map_err(|e| e.to_string())?;
                break;
            }
            _ => {
                // Skip unknown chunk (word-aligned).
                let skip = len + (len & 1);
                let mut sink = vec![0u8; skip.min(1 << 20)];
                let mut left = skip;
                while left > 0 {
                    let n = sink.len().min(left);
                    f.read_exact(&mut sink[..n]).map_err(|e| e.to_string())?;
                    left -= n;
                }
            }
        }
    }
    if fmt_tag != 1 {
        return Err(format!("unsupported WAV format tag {fmt_tag} (PCM only)"));
    }
    if bits != 16 {
        return Err(format!("unsupported sample size {bits} (16-bit only)"));
    }
    if channels == 0 || channels > 2 || data.is_empty() {
        return Err("unsupported channel count or empty data chunk".into());
    }
    Ok(Wav {
        channels,
        rate,
        data,
    })
}

fn sine_tone(freq: f32, secs: f32, rate: u32) -> Vec<u8> {
    let frames = (rate as f32 * secs) as usize;
    let mut out = Vec::with_capacity(frames * 4);
    for i in 0..frames {
        let t = i as f32 / rate as f32;
        // Gentle fade in/out to avoid clicks.
        let env_len = rate as f32 * 0.02;
        let env = (i as f32 / env_len)
            .min((frames - i) as f32 / env_len)
            .min(1.0);
        let s = (2.0 * core::f32::consts::PI * freq * t).sin() * 0.35 * env;
        let v = (s * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes()); // L
        out.extend_from_slice(&v.to_le_bytes()); // R
    }
    out
}

fn main() {
    let mut device = String::from("/dev/dsp");
    let mut tone_hz: Option<f32> = None;
    let mut path: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-d" | "--device" => {
                device = args.next().unwrap_or_else(|| {
                    eprintln!("wavplay: -d needs a device path");
                    exit(2);
                })
            }
            "--tone" => {
                tone_hz = Some(440.0);
                if let Some(next) = args.next() {
                    if let Ok(hz) = next.parse::<f32>() {
                        tone_hz = Some(hz);
                    } else {
                        path = Some(next);
                    }
                }
            }
            "-h" | "--help" => {
                eprintln!("usage: wavplay [-d /dev/dspN] (file.wav | --tone [HZ])");
                exit(0);
            }
            other => path = Some(other.to_string()),
        }
    }

    let (mut pcm, rate, channels) = if let Some(hz) = tone_hz {
        (sine_tone(hz, 3.0, 48000), 48000u32, 2u16)
    } else {
        let p = path.unwrap_or_else(|| {
            eprintln!("usage: wavplay [-d /dev/dspN] (file.wav | --tone [HZ])");
            exit(2);
        });
        let f = File::open(&p).unwrap_or_else(|e| {
            eprintln!("wavplay: {p}: {e}");
            exit(1);
        });
        let wav = parse_wav(f).unwrap_or_else(|e| {
            eprintln!("wavplay: {p}: {e}");
            exit(1);
        });
        (wav.data, wav.rate, wav.channels)
    };

    let mut dev = std::fs::OpenOptions::new()
        .write(true)
        .open(&device)
        .unwrap_or_else(|e| {
            eprintln!("wavplay: {device}: {e}");
            exit(1);
        });
    let fd = dev.as_raw_fd();

    let fmt = ioctl_int(fd, SNDCTL_DSP_SETFMT, AFMT_S16_LE, "SETFMT");
    if fmt != AFMT_S16_LE {
        eprintln!("wavplay: device does not accept S16LE");
        exit(1);
    }
    let dev_ch = ioctl_int(fd, SNDCTL_DSP_CHANNELS, channels as i32, "CHANNELS");
    let dev_rate = ioctl_int(fd, SNDCTL_DSP_SPEED, rate as i32, "SPEED");
    if dev_rate as u32 != rate {
        eprintln!(
            "wavplay: note: device runs at {dev_rate} Hz, source is {rate} Hz (will play off-speed)"
        );
    }

    // The kernel path is stereo; duplicate mono into both channels.
    if channels == 1 && dev_ch == 2 {
        let mut st = Vec::with_capacity(pcm.len() * 2);
        for s in pcm.chunks_exact(2) {
            st.extend_from_slice(s);
            st.extend_from_slice(s);
        }
        pcm = st;
    }

    eprintln!(
        "wavplay: {} -> {device} ({dev_rate} Hz, {dev_ch} ch, {} KiB)",
        if tone_hz.is_some() { "test tone" } else { "wav" },
        pcm.len() / 1024
    );

    let mut off = 0;
    while off < pcm.len() {
        match dev.write(&pcm[off..]) {
            Ok(0) => break,
            Ok(n) => off += n,
            Err(e) => {
                eprintln!("wavplay: write: {e}");
                exit(1);
            }
        }
    }
    unsafe { libc::ioctl(fd, SNDCTL_DSP_SYNC as _, 0) };
}
