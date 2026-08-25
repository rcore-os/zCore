# Audio: HD Audio driver, NVIDIA HDMI, and `/dev/dsp`

Eclipse's audio subsystem is a single Intel HD Audio (HDA) driver that covers
every PCI class-0403 controller:

- the PCH's onboard controller (e.g. `00:1f.3` on X299 boards) with its
  analog codec,
- the HDA function of NVIDIA GPUs (`xx:00.1`) whose codec carries one
  HDMI/DP pin+converter pair per physical connector,
- QEMU's `-device intel-hda -device hda-output`, used to exercise the
  driver in emulation.

## Architecture

```
userspace:  wavplay / mpg123 -o oss / ffmpeg -f oss
                 │ write(2) + OSS ioctls
/dev/dsp[N] ─ linux-object/src/fs/devfs/dsp.rs   (OSS node, one per controller)
                 │ AudioScheme
drivers/src/audio/hda.rs                          (controller + codec + PCM ring)
                 │ CORB/RIRB verbs + stream DMA (all polled, no interrupts)
HDA controller (PCI 04:03) ── codec ── pin ── HDMI/DP or analog jack
```

- **Controller** (`drivers/src/audio/hda.rs`): CRST reset, codec discovery
  via STATESTS, CORB/RIRB rings with polled responses, one output stream
  over a 128 KiB physically contiguous cyclic ring described by a BDL.
  Progress is read from LPIB; consumed ring space is re-zeroed behind the
  DMA position so an underrun plays silence, never stale audio.
- **Codec graph**: the widget walk collects every output-capable pin with a
  reachable converter as a *candidate path*. Path choice is scored (digital
  HDMI/DP pin > presence > ELD valid) and — crucially — **re-evaluated at
  every stream start** (`repick_path`), because on NVIDIA GPUs presence/ELD
  only appear on the pins after the display driver pushes the monitor's ELD,
  long after this driver's PCI probe.
- **HDMI specifics**: digital converter enable, channel-count verb, CEA
  audio infoframe through the pin's DIP buffer, and the NVIDIA coherent-DMA
  (snoop) PCI config bits.

## The display side (NVIDIA GPUs)

The HDA codec alone is not enough on a GPU: the **display engine** must
transmit audio packets for the head, and the codec pin only reports
presence/ELD once someone writes the ELD. Eclipse scans out on the UEFI
GOP's boot modeset, and firmware never enables audio — so the nvidia DRM
driver does it through the RM, piggybacked on its first successful display
query (`rm_enable_hdmi_audio_once` in `drivers/src/display/nvidia.rs`,
backed by `eclipse_rm_hdmi_audio` in `nvidia-rm-sys/vendor/eclipse_rm_init.c`):

1. `GET_EDID_V2` for each connected output;
2. build the ELD (same layout as nvkms `FillELDBuffer`) from the EDID's
   CEA-861 extension — SADs, speaker allocation, monitor name;
3. `NV0073_CTRL_CMD_DFP_SET_ELD_AUDIO_CAPS` with PD=1/ELDV=1, device
   entry 0 — after this the HDA pin reports present + ELD valid;
4. `NV0073_CTRL_CMD_DFP_SET_AUDIO_ENABLE` — audio stream packets on;
5. for HDMI (TMDS) outputs, a General Control Packet un-mute via
   `NV0073_CTRL_CMD_SPECIFIC_SET_OD_PACKET`.

Watch for `[hdmi-audio]` lines in dmesg; `[hda]` lines show each codec's
candidate paths with their live presence/ELD state.

## Userspace API: `/dev/dsp` (OSS)

One node per controller in probe order: `/dev/dsp` (usually the PCH),
`/dev/dsp1`, `/dev/dsp2`, … (the GPU HDMI functions). `write(2)` carries
interleaved S16LE PCM; supported ioctls: `SNDCTL_DSP_SPEED`, `SETFMT`
(S16LE only), `CHANNELS`/`STEREO` (stereo only), `GETBLKSIZE`,
`SETFRAGMENT` (accepted, ignored), `GETFMTS`, `GETOSPACE`, `SYNC`, `POST`,
`RESET`. Writes block (bounded spin-retry) when the ring is full; the
default format is 48 kHz stereo, so `cat music.raw > /dev/dsp` works for
raw 48 kHz S16LE audio.

## Testing

```sh
wavplay --tone                 # 440 Hz sine, 3 s, /dev/dsp
wavplay --tone 880             # another frequency
wavplay -d /dev/dsp1 --tone    # first NVIDIA HDMI codec
wavplay file.wav               # 16-bit PCM WAV (mono is upmixed)
```

`tools/wavplay` is a static musl binary installed into the rootfs by xtask.

In QEMU add `-device intel-hda -device hda-output -audiodev pa,id=snd0
-device hda-output,audiodev=snd0` (or `-audiodev sdl/alsa`) to hear the
guest.

## Known limits

- Playback only (no capture), stereo only, S16LE only.
- The DP audio path uses the same ELD/enable controls but has not been
  exercised; DP-MST audio (device entries > 0) is not implemented.
- The HDMI un-mute GCP is sent once at enable time; a monitor that is
  hot-plugged later gets ELD/PD only when something re-runs the display
  query (`/proc/gpuedid` or a DRM connector rescan).
