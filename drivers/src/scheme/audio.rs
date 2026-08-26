//! PCM audio output device scheme.
//!
//! The model is deliberately small: one always-running (while playing) cyclic
//! DMA ring per device, fed by [`AudioScheme::write`]. The consumer offers
//! bytes; the device accepts as many as currently fit and returns the count —
//! blocking policy (spin, yield, poll) belongs to the caller (the `/dev/dsp`
//! layer), not the driver.
//!
//! Sample format is interleaved little-endian signed 16-bit PCM throughout;
//! rate and channel count are negotiated with [`AudioScheme::set_params`].

use super::Scheme;
use crate::DeviceResult;

pub trait AudioScheme: Scheme {
    /// Negotiate the PCM output format. `rate` in Hz, `channels` interleaved
    /// S16LE. The device picks the nearest configuration it supports and
    /// returns the actual `(rate, channels)` now in effect. Implies
    /// [`reset`](AudioScheme::reset).
    fn set_params(&self, rate: u32, channels: u8) -> DeviceResult<(u32, u8)>;

    /// The `(rate, channels)` currently in effect.
    fn params(&self) -> (u32, u8);

    /// Queue PCM bytes for playback. Returns the number of bytes accepted —
    /// `0` when the ring is full (never an error for a full ring). Starts the
    /// hardware stream if it was stopped.
    fn write(&self, pcm: &[u8]) -> DeviceResult<usize>;

    /// Bytes that can currently be written without truncation.
    fn free_bytes(&self) -> usize;

    /// Total capacity of the playback ring in bytes.
    fn buffer_bytes(&self) -> usize;

    /// Bytes queued but not yet played out.
    fn queued_bytes(&self) -> usize;

    /// Stop playback and drop any queued PCM.
    fn reset(&self) -> DeviceResult;
}
