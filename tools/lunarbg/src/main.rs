//! lunarbg — Eclipse OS's animated wallpaper client (its swaybg replacement).
//!
//! Why not swaybg: Alpine's swaybg decodes wallpapers through gdk-pixbuf,
//! whose loader registry is installed by an apk trigger that may never run
//! under Eclipse OS — leaving it unable to recognise ANY image format. And
//! swaybg is static anyway: lunarbg instead renders the animated cosmic
//! background of the original Eclipse smithay compositor (see `scene.rs`)
//! procedurally at each output's native resolution, over wlr-layer-shell +
//! wl_shm.
//!
//! Animation model: the cosmic base (gradient, stars, grid) is rendered once
//! per size; every animation tick re-renders only the logo region (crescent,
//! orbiting text, arcs, rings, ticks) into one of two persistent shm buffers
//! and damages just that rectangle, so the software-rendered compositor
//! composites a small area per frame, not the whole screen.
//!
//! Pacing: a timer sets the TARGET rate (default 24 fps, `--fps`/`LUNARBG_FPS`
//! 1..=60), but every commit also requests a `wl_surface.frame` callback and
//! the next frame is not rendered until the previous one was consumed. That
//! keeps lunarbg strictly below the compositor's real compositing rate — on a
//! slow software stack the animation degrades gracefully instead of
//! overloading the machine (callback-paced-only rendering at the compositor's
//! full rate once made libinput log "event processing lagging"), and when the
//! wallpaper is fully occluded and the compositor stops asking for frames,
//! rendering drops to a 1 Hz keep-alive.
//!
//! Professional-client details:
//! - per-output state: HiDPI integer scale (`wl_output.scale` +
//!   `wl_surface.set_buffer_scale`), panel aspect from `wl_output.geometry`,
//!   names (`--output NAME` paints selected outputs only);
//! - the surface declares an opaque region, letting the compositor cull
//!   everything beneath the wallpaper;
//! - double buffering that never scribbles over a buffer the compositor still
//!   holds (a tick is skipped instead — dropped frames are invisible, torn
//!   ones are not);
//! - clean shutdown on SIGTERM/SIGINT, pause/resume on SIGUSR1, hot
//!   plug/unplug of outputs (bind + `GlobalRemove`/`Closed`);
//! - `--dump` offscreen render and `--bench` render-loop timing for
//!   regression testing without a compositor. See `--help` for everything.
//!
//! Pure-Rust Wayland stack (wayland-client's Rust backend): a single static
//! musl binary with no runtime library dependencies.

mod fill_guard;
mod par;
mod scene;

use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use wayland_client::{
    backend::ObjectId,
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_output, wl_region, wl_registry, wl_shm,
        wl_shm_pool, wl_surface,
    },
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, ZwlrLayerSurfaceV1},
};

/// Two persistent frame buffers per output, alternated each frame.
const BUFFERS: usize = 2;

/// Default animation rate. The frame-callback gate keeps the real rate at or
/// below what the compositor can actually composite, so this is a target, not
/// a promise; `--fps`/`LUNARBG_FPS` (1..=60) overrides it.
const DEFAULT_FPS: u32 = 24;

/// Set by SIGTERM/SIGINT: leave the main loop and shut down cleanly.
static RUNNING: AtomicBool = AtomicBool::new(true);
/// Set by SIGUSR1: toggle the animation on/off at the next loop turn.
static TOGGLE_ANIMATE: AtomicBool = AtomicBool::new(false);
/// Set by `--quiet`: suppress the setup checkpoints.
static QUIET: AtomicBool = AtomicBool::new(false);

/// Emit a one-time setup checkpoint to stderr so that, if the process crashes,
/// the last line printed pinpoints the stage it died in. Cheap and few (only on
/// the one-time setup path), so on by default; `--quiet` silences it. Defined
/// here, above its first use, because `macro_rules!` is only in scope textually
/// after its definition.
macro_rules! ckpt {
    ($($arg:tt)*) => {{
        if !QUIET.load(Ordering::Relaxed) {
            eprintln!("lunarbg: [ckpt] {}", format_args!($($arg)*));
        }
    }};
}

struct Frames {
    width: usize,
    height: usize,
    /// Integer HiDPI scale these buffers were built at (buffer px per logical
    /// px), needed to translate damage into surface coordinates on a
    /// compositor too old for `wl_surface.damage_buffer`.
    scale: u32,
    layout: scene::Layout,
    base: Vec<u8>,
    /// mmap of the pool holding BUFFERS frames back to back.
    map: *mut u8,
    map_len: usize,
    buffers: [wl_buffer::WlBuffer; BUFFERS],
    busy: [bool; BUFFERS],
    next: usize,
    /// Matches the generation in each buffer's udata: a Release from a
    /// previous generation's buffer (rebuilt after a scale/aspect change)
    /// must not clear the busy flag of the buffer now holding its index.
    generation: u64,
    /// Consecutive ticks skipped because the compositor held both buffers.
    skipped: u32,
}

impl Frames {
    /// Detach the mmap so [`Drop`] will not munmap it — the caller retires
    /// the mapping until the compositor is known to have released the buffers.
    fn take_map(&mut self) -> (*mut u8, usize) {
        let out = (self.map, self.map_len);
        self.map = std::ptr::null_mut();
        self.map_len = 0;
        out
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        for b in &self.buffers {
            b.destroy();
        }
        // Never munmap here: a forgotten `take_map` with live compositor
        // references would UAF. Rebuild / Closed / shutdown always retire via
        // `retire_map`. An unreclaimed map is leaked for the process lifetime.
        if !self.map.is_null() && self.map_len > 0 {
            eprintln!(
                "lunarbg: Frames dropped with unreclaimed mmap ({} bytes); leaking to avoid UAF",
                self.map_len
            );
            self.map = std::ptr::null_mut();
            self.map_len = 0;
        }
    }
}

struct Background {
    surface: wl_surface::WlSurface,
    layer: ZwlrLayerSurfaceV1,
    /// The output this background paints, keyed by proxy id.
    output_id: ObjectId,
    /// Last configure size in LOGICAL pixels (buffer size is this x scale);
    /// kept so a later scale/aspect change can rebuild without a reconfigure.
    logical: (u32, u32),
    /// True when logical came from `Mode::Current` because configure was 0×0.
    /// A later mode change must rebuild without waiting for a new configure.
    size_from_mode: bool,
    /// This layer surface has received its FIRST `configure`. Until then the
    /// protocol forbids committing a buffer — doing so is the fatal
    /// "layer_surface has never been configured" error. The output-event
    /// retry path (`retry_configure_for_output` → `build_frames`) used to
    /// race exactly that: a `wl_output` mode/done event arriving before the
    /// layer surface's own first configure drove a full build + attach +
    /// commit on the unconfigured surface, killing the connection.
    configured: bool,
    /// Last layer-shell configure serial awaiting ack + commit together.
    pending_ack: Option<u32>,
    /// A `wl_surface.frame` callback from the last commit is still pending.
    /// SURFACE state, not buffer state: the callback survives a buffer
    /// rebuild, so keeping this in [`Frames`] would reset the pacing gate on
    /// every scale/aspect change — during occlusion (the exact case the 1 Hz
    /// keep-alive exists for) that meant full-rate rendering until the next
    /// repaint, and a second stacked callback besides.
    pending_cb: bool,
    /// The compositor has delivered at least one frame callback, so gating on
    /// them is known to be safe (a compositor that never delivers any would
    /// otherwise freeze the animation).
    saw_cb: bool,
    /// When the pending commit was made, for the 1 Hz occluded keep-alive.
    committed_at: Instant,
    /// Bumped when this Background is created; stamped into frame-callback
    /// udata so a late Done after destroy/recreate cannot open pacing on a
    /// recycled protocol id.
    surface_gen: u64,
    /// A tick was skipped because both buffers were busy — repaint on Release.
    dirty: bool,
    frames: Option<Frames>,
}

/// Post surface damage for a rect given in BUFFER pixels.
///
/// `wl_surface.damage_buffer` is a version-4 request, but the surface's
/// version is whatever `wl_compositor` advertised — the code already gates
/// `set_buffer_scale` on version 3, and sending a request the object does not
/// support is a fatal protocol error, so a compositor advertising
/// `wl_compositor` v1..=3 would kill the wallpaper outright (black desktop)
/// rather than merely lose a feature. Fall back to the v1 `damage`, which
/// takes SURFACE coordinates — hence the divide by the buffer scale (rounded
/// outwards so the damage never under-covers the repainted pixels).
fn damage(surface: &wl_surface::WlSurface, scale: u32, x: i32, y: i32, w: i32, h: i32) {
    if surface.version() >= 4 {
        surface.damage_buffer(x, y, w, h);
        return;
    }
    let s = scale.max(1) as i32;
    // All four are non-negative here (buffer-space rects), so the usual
    // round-up idiom is safe; i32::div_ceil is still unstable.
    let (x0, y0) = (x / s, y / s);
    let (x1, y1) = ((x + w + s - 1) / s, (y + h + s - 1) / s);
    surface.damage(x0, y0, x1 - x0, y1 - y0);
}

/// Everything we track per `wl_output` global.
struct OutputInfo {
    output: wl_output::WlOutput,
    /// The registry name, to match `GlobalRemove` on unplug.
    global_name: u32,
    /// `wl_output.name` (v4+), for `--output NAME` selection.
    name: Option<String>,
    /// Integer HiDPI scale from `wl_output.scale`.
    scale: i32,
    /// Physical panel aspect (width/height) from `wl_output.geometry`, used to
    /// draw circles round even when the driver's mode is not the panel's
    /// native aspect. `None` until the output reports a sane physical size.
    aspect: Option<f32>,
    /// Last `Mode::Current` size (for configure width/height == 0).
    mode_w: u32,
    mode_h: u32,
    /// Scale/geometry changed; rebuild once on `wl_output.Done`.
    pending_rebuild: bool,
    /// A create-surface decision was made (surface created, or filtered out
    /// by `--output`), so `ensure_surfaces` must not revisit this output.
    claimed: bool,
    /// Layer surface received `Closed` while the output remains: do not
    /// recreate (avoids Closed→create→Closed loops). Cleared only when the
    /// output global is removed.
    closed: bool,
}

/// Old shm mapping held until its generation's busy buffers have all Released.
struct RetiredMap {
    ptr: *mut u8,
    len: usize,
    generation: u64,
    pending: u32,
}

#[derive(Default)]
struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
    outputs: Vec<OutputInfo>,
    backgrounds: Vec<Background>,
    start: Option<Instant>,
    animate: bool,
    /// `--aspect` / `LUNARBG_ASPECT`; takes priority over `wl_output.geometry`
    /// so fabricated DRM millimetres cannot cancel the packaging override.
    aspect_cli: Option<f32>,
    /// `--output NAME` filters; empty = paint every output.
    only_outputs: Vec<String>,
    warned_unnamed: bool,
    /// Bumped on every build_frames, stamped into buffer udata.
    generation: u64,
    /// Bumped when a Background is created; pairs with frame-callback udata.
    surface_generation: u64,
    /// Old shm mappings awaiting Release-counted munmap after a resize.
    retired_maps: Vec<RetiredMap>,
    /// Socket write buffer was full — skip ticks until a flush succeeds.
    flush_blocked: bool,
}

impl State {
    fn now_ms(&mut self) -> u64 {
        let start = self.start.get_or_insert_with(Instant::now);
        start.elapsed().as_millis() as u64
    }

    fn output_info(&self, id: &ObjectId) -> Option<&OutputInfo> {
        self.outputs.iter().find(|o| o.output.id() == *id)
    }

    /// Create background surfaces for any outputs that appeared once the
    /// required globals are all bound. Safe to call repeatedly.
    fn ensure_surfaces(&mut self, qh: &QueueHandle<State>) {
        let (Some(compositor), Some(layer_shell)) = (&self.compositor, &self.layer_shell) else {
            return;
        };
        for oi in self.outputs.iter_mut().filter(|o| !o.claimed && !o.closed) {
            if !self.only_outputs.is_empty() {
                match &oi.name {
                    Some(n) if self.only_outputs.iter().any(|f| f == n) => {}
                    Some(_) => {
                        // Named, and not one of ours: decided, skip for good.
                        oi.claimed = true;
                        continue;
                    }
                    None => {
                        // wl_output v4 sends the name right after bind; wait
                        // for it. Older servers never will — say so once.
                        if oi.output.version() < 4 && !self.warned_unnamed {
                            eprintln!(
                                "lunarbg: --output given but the compositor's wl_output \
                                 is too old to report names; those outputs stay unpainted"
                            );
                            self.warned_unnamed = true;
                        }
                        continue;
                    }
                }
            }
            if self.backgrounds.len() >= fill_guard::MAX_BACKGROUNDS {
                eprintln!(
                    "lunarbg: background list at MAX_BACKGROUNDS={}; will retry later",
                    fill_guard::MAX_BACKGROUNDS
                );
                // Do NOT mark claimed — when a slot frees (unplug/Closed) we
                // must be able to create a surface for this output.
                continue;
            }
            let surface = compositor.create_surface(qh, ());
            let layer = layer_shell.get_layer_surface(
                &surface,
                Some(&oi.output),
                zwlr_layer_shell_v1::Layer::Background,
                "wallpaper".into(),
                qh,
                (),
            );
            layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
            layer.set_exclusive_zone(-1);
            layer.set_size(0, 0);
            // A wl_surface's input region defaults to INFINITE, and the
            // layer-shell spec says a surface that does not want input must
            // set an empty one. Without this the wallpaper — anchored to all
            // four edges, so covering the whole output — swallowed every
            // pointer and touch event on the empty desktop: labwc saw the
            // layer surface instead of its Root context, so right-clicking
            // the desktop never opened the root menu (the app launcher this
            // image binds it to), and the cursor image was left to a client
            // that binds no wl_seat and can never set one.
            let empty = compositor.create_region(qh, ());
            surface.set_input_region(Some(&empty));
            empty.destroy();
            surface.commit();
            oi.claimed = true;
            self.surface_generation = self.surface_generation.wrapping_add(1).max(1);
            let surface_gen = self.surface_generation;
            self.backgrounds.push(Background {
                surface,
                layer,
                output_id: oi.output.id(),
                logical: (0, 0),
                size_from_mode: false,
                configured: false,
                pending_ack: None,
                pending_cb: false,
                saw_cb: false,
                committed_at: Instant::now(),
                surface_gen,
                dirty: false,
                frames: None,
            });
        }
    }

    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.generation
    }

    /// Retire an shm mapping until `pending` Releases of `generation` arrive.
    /// Force-unmap only applies to entries already drained to `pending == 0`.
    fn retire_map(&mut self, map: *mut u8, map_len: usize, generation: u64, pending: u32) {
        if map.is_null() || map_len == 0 {
            return;
        }
        if pending == 0 {
            unsafe { libc::munmap(map as *mut libc::c_void, map_len) };
            return;
        }
        self.retired_maps.push(RetiredMap {
            ptr: map,
            len: map_len,
            generation,
            pending,
        });
        self.gc_retired_maps();
    }

    fn note_retired_release(&mut self, generation: u64) {
        if let Some(rm) = self
            .retired_maps
            .iter_mut()
            .find(|r| r.generation == generation && r.pending > 0)
        {
            rm.pending -= 1;
        }
        self.gc_retired_maps();
    }

    fn gc_retired_maps(&mut self) {
        let mut i = 0;
        while i < self.retired_maps.len() {
            if self.retired_maps[i].pending == 0 {
                let rm = self.retired_maps.remove(i);
                unsafe { libc::munmap(rm.ptr as *mut libc::c_void, rm.len) };
            } else {
                i += 1;
            }
        }
        // Soft cap: never force-unmap a map that still has pending Releases.
        // Drop the oldest zero-pending leftovers first (already handled); if
        // still over the cap, log — better to hold memory than UAF.
        if self.retired_maps.len() > fill_guard::MAX_RETIRED_MAPS {
            eprintln!(
                "lunarbg: {} retired maps awaiting Release (cap {}); holding to avoid UAF",
                self.retired_maps.len(),
                fill_guard::MAX_RETIRED_MAPS
            );
        }
    }

    fn take_frames_for_retire(frames: &mut Frames) -> (u64, u32, *mut u8, usize) {
        let generation = frames.generation;
        let pending = frames.busy.iter().filter(|&&b| b).count() as u32;
        let (ptr, len) = frames.take_map();
        (generation, pending, ptr, len)
    }

    fn bg_index_by_layer(&self, layer_id: u32) -> Option<usize> {
        self.backgrounds
            .iter()
            .position(|b| b.layer.id().protocol_id() == layer_id)
    }

    fn ack_pending(bg: &mut Background) {
        if let Some(serial) = bg.pending_ack.take() {
            bg.layer.ack_configure(serial);
        }
    }

    /// Store the configured LOGICAL size, then (re)build the buffers.
    /// Ack is deferred until a successful commit inside [`build_frames`].
    fn configure(&mut self, qh: &QueueHandle<State>, layer_id: u32, mut w: u32, mut h: u32) {
        let Some(idx) = self.bg_index_by_layer(layer_id) else {
            return;
        };
        let from_mode = w == 0 || h == 0;
        // Protocol: 0 means "client decides" — use the last Mode::Current.
        if w == 0 || h == 0 {
            let out_id = self.backgrounds[idx].output_id.clone();
            if let Some(oi) = self.output_info(&out_id) {
                if w == 0 {
                    w = oi.mode_w;
                }
                if h == 0 {
                    h = oi.mode_h;
                }
            }
        }
        if w == 0 || h == 0 {
            // Still unknown — keep pending_ack until a real mode arrives.
            return;
        }
        let prev = self.backgrounds[idx].logical;
        let prev_from_mode = self.backgrounds[idx].size_from_mode;
        self.backgrounds[idx].logical = (w, h);
        self.backgrounds[idx].size_from_mode = from_mode;
        if !self.build_frames(qh, idx, false) {
            self.backgrounds[idx].logical = prev;
            self.backgrounds[idx].size_from_mode = prev_from_mode;
        }
    }

    /// An output's scale or aspect changed after mapping: rebuild its
    /// background at the same logical size. `force` bypasses the same-size
    /// early-out (an aspect change keeps the buffer size but moves pixels).
    fn rebuild_output(&mut self, output_id: &ObjectId, qh: &QueueHandle<State>) {
        if let Some(idx) = self
            .backgrounds
            .iter()
            .position(|b| b.output_id == *output_id && b.frames.is_some())
        {
            let _ = self.build_frames(qh, idx, true);
        }
    }

    /// Retry a configure that was waiting on mode or a failed build.
    fn retry_configure_for_output(&mut self, output_id: &ObjectId, qh: &QueueHandle<State>) {
        let Some(idx) = self
            .backgrounds
            .iter()
            .position(|b| b.output_id == *output_id)
        else {
            return;
        };
        let layer_id = self.backgrounds[idx].layer.id().protocol_id();
        let pending = self.backgrounds[idx].pending_ack.is_some();
        let needs = pending
            || self.backgrounds[idx].frames.is_none()
            || self.backgrounds[idx].size_from_mode;
        if !needs {
            return;
        }
        // Pass 0,0 so configure re-reads Mode::Current when size_from_mode.
        let (w, h) = if self.backgrounds[idx].size_from_mode || self.backgrounds[idx].logical == (0, 0)
        {
            (0, 0)
        } else {
            self.backgrounds[idx].logical
        };
        self.configure(qh, layer_id, w, h);
    }

    /// Apply pending scale/aspect/mode rebuilds (and mode-driven reconfigure).
    fn apply_pending_output(&mut self, output_id: &ObjectId, qh: &QueueHandle<State>) {
        let rebuild = self
            .outputs
            .iter_mut()
            .find(|o| o.output.id() == *output_id)
            .map(|o| {
                let r = o.pending_rebuild;
                o.pending_rebuild = false;
                r
            })
            .unwrap_or(false);
        self.retry_configure_for_output(output_id, qh);
        if rebuild {
            // size_from_mode: logical must track the new mode before rebuild.
            let mode = self
                .outputs
                .iter()
                .find(|o| o.output.id() == *output_id)
                .map(|o| (o.mode_w, o.mode_h));
            if let Some((mw, mh)) = mode {
                if mw > 0 && mh > 0 {
                    if let Some(bg) = self
                        .backgrounds
                        .iter_mut()
                        .find(|b| b.output_id == *output_id && b.size_from_mode)
                    {
                        bg.logical = (mw, mh);
                    }
                }
            }
            self.rebuild_output(output_id, qh);
        }
    }

    /// (Re)build the per-size resources after a configure or an output change.
    /// Returns false if nothing was committed (caller should keep pending_ack).
    fn build_frames(&mut self, qh: &QueueHandle<State>, idx: usize, force: bool) -> bool {
        let t_ms = self.now_ms();
        // Protocol invariant: no buffer may be committed before this surface's
        // FIRST configure (fatal "layer_surface has never been configured").
        // Every builder funnels through here, so this single guard covers the
        // configure-event path, the output-event retry and the rebuilds alike;
        // the pending state is kept and the real configure re-drives us.
        if !self.backgrounds[idx].configured {
            return false;
        }
        let (lw, lh) = self.backgrounds[idx].logical;
        if lw == 0 || lh == 0 {
            return false; // not configured yet
        }
        let layer_id = self.backgrounds[idx].layer.id().protocol_id();
        let out_id = self.backgrounds[idx].output_id.clone();
        let surface_ver = self.backgrounds[idx].surface.version();
        // Integer HiDPI: render at scale x the logical size and announce it
        // with set_buffer_scale (a wl_surface v3+ request), so text and rings
        // stay crisp instead of being upscaled by the compositor.
        let info_scale = self
            .output_info(&out_id)
            .map(|o| o.scale)
            .unwrap_or(1);
        let info_aspect = self.output_info(&out_id).and_then(|o| o.aspect);
        // Clamp the advertised scale defensively: a buggy compositor claiming
        // an absurd factor must not blow the buffer-size math up. Also drop
        // scale until the buffer fits MAX_BUFFER_DIM instead of skipping.
        let mut scale = if surface_ver >= 3 {
            info_scale.clamp(1, 8) as u32
        } else {
            1
        };
        while scale > 1
            && (lw.saturating_mul(scale) > fill_guard::MAX_BUFFER_DIM
                || lh.saturating_mul(scale) > fill_guard::MAX_BUFFER_DIM)
        {
            scale -= 1;
        }
        // CLI / LUNARBG_ASPECT override geometry: fabricated DRM mm must not
        // lock out the packaging knob for Eclipse panels.
        let aspect = self.aspect_cli.or(info_aspect);
        let (w, h) = (lw as usize * scale as usize, lh as usize * scale as usize);
        let same = self.backgrounds[idx]
            .frames
            .as_ref()
            .is_some_and(|f| f.width == w && f.height == h && !force);
        if same {
            Self::ack_pending(&mut self.backgrounds[idx]);
            self.backgrounds[idx].surface.commit();
            return true;
        }
        let Some(shm) = self.shm.clone() else {
            return false;
        };

        // wl_shm sizes travel as i32: a pool past that (a 16K output, or 8K
        // at 2x scale) must be refused, and the size math itself is checked so
        // a hostile configure cannot wrap it past the guard in release builds.
        let Some(total) = w
            .checked_mul(4)
            .and_then(|stride| stride.checked_mul(h))
            .and_then(|frame| frame.checked_mul(BUFFERS))
            .filter(|t| *t <= i32::MAX as usize)
        else {
            eprintln!(
                "lunarbg: {w}x{h} needs a bigger pool than wl_shm can address; skipping output"
            );
            return false;
        };
        if w > fill_guard::MAX_BUFFER_DIM as usize || h > fill_guard::MAX_BUFFER_DIM as usize {
            eprintln!(
                "lunarbg: {w}x{h} past MAX_BUFFER_DIM={}; skipping output",
                fill_guard::MAX_BUFFER_DIM
            );
            return false;
        }
        if w.saturating_mul(h) > fill_guard::MAX_BUFFER_PIXELS {
            eprintln!(
                "lunarbg: {w}x{h} is {} Mpx, past the {} Mpx render limit; skipping output",
                w.saturating_mul(h) >> 20,
                fill_guard::MAX_BUFFER_PIXELS >> 20
            );
            return false;
        }
        let stride = w * 4;
        let frame_size = stride * h;
        ckpt!("configure {w}x{h} (scale {scale}): allocating shm pool total={total}");

        let raw = unsafe {
            libc::memfd_create(
                b"lunarbg\0".as_ptr() as *const libc::c_char,
                libc::MFD_CLOEXEC,
            )
        };
        if raw < 0 {
            eprintln!("lunarbg: memfd_create failed");
            return false;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        if unsafe { libc::ftruncate(raw, total as libc::off_t) } != 0 {
            eprintln!("lunarbg: ftruncate({total}) failed");
            return false;
        }
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                total,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                raw,
                0,
            )
        };
        if map == libc::MAP_FAILED {
            eprintln!("lunarbg: mmap failed");
            return false;
        }
        let map = map as *mut u8;

        let generation = self.next_generation();
        let pool = shm.create_pool(fd.as_fd(), total as i32, qh, ());
        let make = |i: usize| {
            pool.create_buffer(
                (i * frame_size) as i32,
                w as i32,
                h as i32,
                stride as i32,
                wl_shm::Format::Xrgb8888,
                qh,
                (layer_id, i, generation),
            )
        };
        let buffers = [make(0), make(1)];
        // The pool object can go away; buffers keep the storage alive
        // server-side and the mapping outlives the closed fd.
        pool.destroy();

        ckpt!("configure {w}x{h}: mmap ok; rendering base scene");
        let layout = scene::layout(w, h, aspect, scale);
        let Some(base) = scene::render_base(w, h, aspect, scale) else {
            eprintln!("lunarbg: render_base failed for {w}x{h} (alloc)");
            for b in &buffers {
                b.destroy();
            }
            unsafe { libc::munmap(map as *mut libc::c_void, total) };
            return false;
        };

        // Seed BOTH buffers with the full base scene. Only buffer 0 used to
        // get it; buffer 1 stayed zeroed (memfd), and since ticks repaint just
        // the logo region, every frame shown from buffer 1 had BLACK outside
        // the logo — on the real monitor the wallpaper alternated between the
        // full cosmic scene and a dark screen with a floating square.
        ckpt!("configure {w}x{h}: first write to mmap'd memfd (buffer 0)");
        let frame0: &mut [u8] = unsafe { std::slice::from_raw_parts_mut(map, frame_size) };
        frame0.copy_from_slice(&base);
        scene::render_frame(frame0, w, &base, &layout, t_ms);
        let frame1: &mut [u8] =
            unsafe { std::slice::from_raw_parts_mut(map.add(frame_size), frame_size) };
        frame1.copy_from_slice(&base);
        ckpt!("configure {w}x{h}: buffers seeded; committing surface");

        let compositor = self.compositor.clone();
        let old_retire = {
            let bg = &mut self.backgrounds[idx];
            bg.frames.take().map(|mut old| {
                let r = Self::take_frames_for_retire(&mut old);
                drop(old); // destroy wl_buffers; map already detached
                r
            })
        };
        {
            let bg = &mut self.backgrounds[idx];
            bg.dirty = false;
            bg.frames = Some(Frames {
                width: w,
                height: h,
                scale,
                layout,
                base,
                map,
                map_len: total,
                buffers,
                busy: [true, false],
                next: 1,
                generation,
                skipped: 0,
            });
        }
        if let Some((gen, pending, p, l)) = old_retire {
            self.retire_map(p, l, gen, pending);
        }
        let bg = &mut self.backgrounds[idx];
        Self::ack_pending(bg);
        if bg.surface.version() >= 3 {
            bg.surface.set_buffer_scale(scale as i32);
        }
        // The wallpaper is fully opaque (XRGB): declaring it lets the
        // compositor skip blending and cull everything beneath the surface.
        if let Some(compositor) = &compositor {
            let region = compositor.create_region(qh, ());
            region.add(0, 0, lw as i32, lh as i32);
            bg.surface.set_opaque_region(Some(&region));
            region.destroy();
        }
        let Some(buf0) = bg.frames.as_ref().map(|f| f.buffers[0].clone()) else {
            return false;
        };
        bg.surface.attach(Some(&buf0), 0, 0);
        damage(&bg.surface, scale, 0, 0, w as i32, h as i32);
        bg.surface.commit();
        true
    }

    /// One animation step for a background, driven by the main loop's timer
    /// (NOT compositor frame callbacks alone: callback-paced rendering ran at
    /// the compositor's full rate, and on this software-rendered stack that
    /// overloaded the machine — libinput logged "event processing lagging,
    /// your system is too slow" right after session start). The timer sets
    /// the ceiling; the frame callback of the previous commit gates below it.
    fn tick(&mut self, qh: &QueueHandle<State>, layer_id: u32) {
        let t_ms = self.now_ms();
        let Some(idx) = self.bg_index_by_layer(layer_id) else {
            return;
        };
        let bg = &mut self.backgrounds[idx];
        let Some(frames) = &mut bg.frames else { return };

        // Frame-callback gate: once the compositor is known to deliver frame
        // callbacks, never render ahead of it. If the wallpaper is occluded
        // and the compositor stops asking for frames entirely, fall back to a
        // 1 Hz keep-alive so the clock stays current and a compositor that
        // silently dropped one callback can't freeze the animation. This is
        // surface state on `bg`, so a buffer rebuild does not reset the gate.
        if bg.pending_cb && bg.saw_cb && bg.committed_at.elapsed() < Duration::from_secs(1) {
            return;
        }

        // Pick a released buffer. Never write a buffer the compositor still
        // holds — mark dirty and repaint from the Release handler instead.
        let i = if !frames.busy[frames.next] {
            frames.next
        } else if !frames.busy[1 - frames.next] {
            1 - frames.next
        } else {
            frames.skipped = frames.skipped.saturating_add(1);
            bg.dirty = true;
            return;
        };
        frames.skipped = 0;
        bg.dirty = false;
        frames.next = 1 - i;

        let Some(frame_size) = frames
            .width
            .checked_mul(frames.height)
            .and_then(|n| n.checked_mul(4))
        else {
            return;
        };
        // Mark busy only after size is known — an early return must not strand
        // the slot forever.
        frames.busy[i] = true;
        let frame: &mut [u8] =
            unsafe { std::slice::from_raw_parts_mut(frames.map.add(i * frame_size), frame_size) };
        // The buffer alternates, so it carries a stale logo region from two
        // frames ago; render_frame restores that region from the base first.
        scene::render_frame(frame, frames.width, &frames.base, &frames.layout, t_ms);

        let (rx, ry, rw, rh) = frames.layout.region;
        let scale = frames.scale;
        bg.surface.attach(Some(&frames.buffers[i]), 0, 0);
        damage(&bg.surface, scale, rx as i32, ry as i32, rw as i32, rh as i32);
        // At most ONE outstanding frame callback per surface: while the
        // wallpaper is occluded and the compositor withholds Done events, the
        // 1 Hz keep-alive would otherwise stack a fresh never-firing callback
        // onto the surface every second, growing both sides' object maps
        // without bound. The single pending callback is enough to re-open the
        // gate the moment the wallpaper is visible again.
        if !bg.pending_cb {
            let gen = bg.surface_gen;
            bg.surface.frame(qh, (layer_id, gen));
            bg.pending_cb = true;
        }
        bg.committed_at = Instant::now();
        bg.surface.commit();
    }

    /// Render a tick on every configured background.
    fn tick_all(&mut self, qh: &QueueHandle<State>) {
        for idx in 0..self.backgrounds.len() {
            if self.backgrounds[idx].frames.is_some() {
                let id = self.backgrounds[idx].layer.id().protocol_id();
                self.tick(qh, id);
            }
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, 1, qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_output" => {
                    if state.outputs.len() >= fill_guard::MAX_OUTPUTS {
                        eprintln!(
                            "lunarbg: ignoring wl_output past MAX_OUTPUTS={}",
                            fill_guard::MAX_OUTPUTS
                        );
                    } else {
                        let output: wl_output::WlOutput =
                            registry.bind(name, version.min(4), qh, ());
                        state.outputs.push(OutputInfo {
                            output,
                            global_name: name,
                            name: None,
                            scale: 1,
                            aspect: None,
                            mode_w: 0,
                            mode_h: 0,
                            pending_rebuild: false,
                            claimed: false,
                            closed: false,
                        });
                    }
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                // Output unplugged: tear down its background and release our
                // binding. Doing the teardown here rather than trusting the
                // layer surface's Closed event matters — Closed is not
                // guaranteed to arrive (nor to arrive first), and without it
                // the Background lived on forever, rendering and committing
                // every tick to a surface whose output was gone.
                if let Some(pos) = state.outputs.iter().position(|o| o.global_name == name) {
                    let oi = state.outputs.remove(pos);
                    let out_id = oi.output.id();
                    if let Some(bpos) = state.backgrounds.iter().position(|b| b.output_id == out_id)
                    {
                        let mut bg = state.backgrounds.remove(bpos);
                        bg.layer.destroy();
                        bg.surface.destroy();
                        if let Some(mut frames) = bg.frames.take() {
                            let (gen, pending, p, l) = State::take_frames_for_retire(&mut frames);
                            drop(frames);
                            state.retire_map(p, l, gen, pending);
                        }
                    }
                    if oi.output.version() >= 3 {
                        oi.output.release();
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                // Defer ack until a matching commit succeeds (build_frames).
                // Acking then failing to map left the layer black permanently.
                if let Some(idx) = state.bg_index_by_layer(layer.id().protocol_id()) {
                    state.backgrounds[idx].configured = true;
                    state.backgrounds[idx].pending_ack = Some(serial);
                }
                state.configure(qh, layer.id().protocol_id(), width, height);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                let id = layer.id().protocol_id();
                if let Some(pos) = state.bg_index_by_layer(id) {
                    let mut bg = state.backgrounds.remove(pos);
                    let out_id = bg.output_id.clone();
                    bg.layer.destroy();
                    bg.surface.destroy();
                    if let Some(mut frames) = bg.frames.take() {
                        let (gen, pending, p, l) = State::take_frames_for_retire(&mut frames);
                        drop(frames);
                        state.retire_map(p, l, gen, pending);
                    }
                    // Keep claimed + mark closed so ensure_surfaces does not
                    // recreate into a Closed→create→Closed loop.
                    if let Some(oi) = state.outputs.iter_mut().find(|o| o.output.id() == out_id) {
                        oi.claimed = true;
                        oi.closed = true;
                    }
                }
            }
            _ => {}
        }
    }
}

/// Buffers: udata is (layer id, buffer index, generation), to clear the busy
/// flag. The generation check drops stale Releases from buffers that were
/// rebuilt away (scale/aspect change) — without it a late Release for an old
/// buffer would mark the NEW buffer at the same index reusable while the
/// compositor still displays it.
impl Dispatch<wl_buffer::WlBuffer, (u32, usize, u64)> for State {
    fn event(
        state: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        (layer_id, i, generation): &(u32, usize, u64),
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        if let wl_buffer::Event::Release = event {
            let Some(idx) = state.bg_index_by_layer(*layer_id) else {
                // Background gone — still count toward a retired map.
                state.note_retired_release(*generation);
                return;
            };
            let mut need_tick = false;
            let mut stale = false;
            {
                let bg = &mut state.backgrounds[idx];
                if let Some(frames) = &mut bg.frames {
                    if frames.generation == *generation {
                        if let Some(slot) = frames.busy.get_mut(*i) {
                            *slot = false;
                        }
                        need_tick = bg.dirty;
                    } else {
                        stale = true;
                    }
                } else {
                    stale = true;
                }
            }
            if stale {
                state.note_retired_release(*generation);
            }
            // Do not enqueue attach/commit while the socket cannot accept writes.
            if need_tick && !state.flush_blocked {
                state.tick(qh, *layer_id);
            } else if need_tick {
                if let Some(bg) = state.backgrounds.get_mut(idx) {
                    bg.dirty = true;
                }
            }
        }
    }
}

/// Frame callbacks: udata is (layer id, surface_gen); clears the pacing gate.
impl Dispatch<wl_callback::WlCallback, (u32, u64)> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        (layer_id, surface_gen): &(u32, u64),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            if let Some(idx) = state.bg_index_by_layer(*layer_id) {
                let bg = &mut state.backgrounds[idx];
                if bg.surface_gen != *surface_gen {
                    return; // stale callback after Closed/recreate
                }
                bg.pending_cb = false;
                bg.saw_cb = true;
            }
        }
    }
}

// Globals whose events carry nothing we need.
wayland_client::delegate_noop!(State: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(State: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(State: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(State: ignore wl_region::WlRegion);

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        let id = output.id();
        match event {
            // The panel's physical size (mm) gives its true aspect ratio, so
            // lunarbg draws round circles regardless of whether the driver's
            // mode matches the panel's native aspect. Guard against panels
            // that report an unknown (0) or nonsensical physical size.
            wl_output::Event::Geometry {
                physical_width,
                physical_height,
                transform,
                ..
            } if physical_width > 0 && physical_height > 0 => {
                // physical_width/height describe the UNROTATED panel. On a
                // 90/270-rotated output the framebuffer's axes are swapped
                // relative to it, so the aspect the logo must pre-compensate
                // for is the inverse — without this the circles drew as a
                // 2:1 ellipse on any portrait-rotated monitor.
                let quarter_turn = matches!(
                    transform,
                    WEnum::Value(
                        wl_output::Transform::_90
                            | wl_output::Transform::_270
                            | wl_output::Transform::Flipped90
                            | wl_output::Transform::Flipped270
                    )
                );
                let (pw, ph) = if quarter_turn {
                    (physical_height, physical_width)
                } else {
                    (physical_width, physical_height)
                };
                let aspect = pw as f32 / ph as f32;
                if let Some(info) = state.outputs.iter_mut().find(|o| o.output.id() == id) {
                    if info.aspect != Some(aspect) {
                        info.aspect = Some(aspect);
                        info.pending_rebuild = true;
                    }
                }
                if output.version() < 2 {
                    state.apply_pending_output(&id, qh);
                }
            }
            wl_output::Event::Scale { factor } => {
                let factor = factor.max(1);
                if let Some(info) = state.outputs.iter_mut().find(|o| o.output.id() == id) {
                    if info.scale != factor {
                        info.scale = factor;
                        info.pending_rebuild = true;
                    }
                }
                if output.version() < 2 {
                    state.apply_pending_output(&id, qh);
                }
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                let flags = match flags {
                    WEnum::Value(f) => f,
                    _ => return,
                };
                if flags.contains(wl_output::Mode::Current) {
                    let nw = width.max(0) as u32;
                    let nh = height.max(0) as u32;
                    let mut changed = false;
                    if let Some(info) = state.outputs.iter_mut().find(|o| o.output.id() == id) {
                        if info.mode_w != nw || info.mode_h != nh {
                            info.mode_w = nw;
                            info.mode_h = nh;
                            info.pending_rebuild = true;
                            changed = true;
                        }
                    }
                    if changed {
                        if output.version() < 2 {
                            state.apply_pending_output(&id, qh);
                        } else {
                            // Mode may unlock a configure(0,0) waiting on size.
                            state.retry_configure_for_output(&id, qh);
                        }
                    }
                }
            }
            wl_output::Event::Done => {
                state.apply_pending_output(&id, qh);
            }
            wl_output::Event::Name { name } => {
                if let Some(info) = state.outputs.iter_mut().find(|o| o.output.id() == id) {
                    info.name = Some(name);
                }
            }
            _ => {}
        }
    }
}
wayland_client::delegate_noop!(State: ignore ZwlrLayerShellV1);

// The WEnum import keeps signatures readable if event matching grows later.
#[allow(unused_imports)]
use WEnum as _;

/// Connect to the Wayland compositor, auto-detecting the socket when the
/// environment does not point at one.
///
/// `Connection::connect_to_env()` — like every wayland-client program — needs
/// `WAYLAND_DISPLAY` set and resolves it under `XDG_RUNTIME_DIR`. Two common
/// Eclipse-OS situations leave those unset even though labwc is running:
///   * launched from a bare init/VT shell that never sourced `/etc/profile`,
///     so no `XDG_*` is exported;
///   * labwc's autostart exports `XDG_RUNTIME_DIR` but not `WAYLAND_DISPLAY` —
///     libwayland clients (foot) fall back to `wayland-0` and connect, but the
///     pure-Rust wayland-client refuses without the variable. That is exactly
///     why lunarbg/lunarbar died in autostart while foot ran fine.
///
/// So: try the standard connect first; on failure, scan the usual runtime
/// directories for a live `wayland-N` socket and connect to it directly,
/// defaulting to `wayland-0` like libwayland. On success we publish
/// `XDG_RUNTIME_DIR`/`WAYLAND_DISPLAY` into our own environment so anything we
/// later spawn inherits a working session.
fn connect_wayland() -> Result<Connection, String> {
    use std::path::PathBuf;

    // 1) Standard path: honours WAYLAND_SOCKET / WAYLAND_DISPLAY / XDG_RUNTIME_DIR.
    if let Ok(c) = Connection::connect_to_env() {
        return Ok(c);
    }

    // 2) Auto-detect. Probe runtime dirs, most specific first.
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(d);
        if p.is_absolute() {
            dirs.push(p);
        }
    }
    // Eclipse OS runs as root, so /run/user/0 is the default XDG_RUNTIME_DIR.
    for d in ["/run/user/0", "/run/user/1000", "/tmp"] {
        let p = PathBuf::from(d);
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }

    // A relative WAYLAND_DISPLAY name (without XDG_RUNTIME_DIR) still guides us.
    let hinted = std::env::var("WAYLAND_DISPLAY")
        .ok()
        .filter(|h| !h.contains('/'));

    for dir in &dirs {
        // Candidate socket names: the hint, wayland-0.., plus any wayland-*
        // the directory actually contains.
        let mut names: Vec<String> = Vec::new();
        if let Some(ref h) = hinted {
            names.push(h.clone());
        }
        for i in 0..8 {
            names.push(format!("wayland-{i}"));
        }
        if let Ok(rd) = std::fs::read_dir(dir) {
            for ent in rd.flatten() {
                if let Ok(n) = ent.file_name().into_string() {
                    if n.starts_with("wayland-") && !n.ends_with(".lock") && !names.contains(&n) {
                        names.push(n);
                    }
                }
            }
        }
        for name in &names {
            let path = dir.join(name);
            if let Ok(stream) = std::os::unix::net::UnixStream::connect(&path) {
                if let Ok(conn) = Connection::from_socket(stream) {
                    // Publish a working session for any child we spawn.
                    std::env::set_var("XDG_RUNTIME_DIR", dir);
                    std::env::set_var("WAYLAND_DISPLAY", name);
                    return Ok(conn);
                }
            }
        }
    }
    Err("Could not find a running Wayland compositor (is labwc started?)".into())
}

/// Install a crash handler that reports the faulting address on
/// SIGSEGV/SIGBUS/SIGILL before dying, so a crash on real hardware is
/// self-diagnosing (no dmesg needed). The handler uses only `write(2)` and
/// manual hex formatting — both async-signal-safe — then restores the default
/// disposition and re-raises so the shell still sees the real signal exit code.
fn install_crash_handler() {
    extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, _ctx: *mut libc::c_void) {
        // A kernel that does not fully honour SA_SIGINFO hands the handler a
        // null siginfo — and Eclipse OS's sigaction is already known to
        // mishandle pointers (see the oldact note below). Dereferencing it
        // here would fault INSIDE the SIGSEGV handler, losing the very
        // diagnostic this exists to print. Report a zero address instead.
        let addr = if info.is_null() {
            0
        } else {
            (unsafe { (*info).si_addr() }) as usize
        };
        // "lunarbg: FATAL signal SS fault-addr 0xHHHHHHHHHHHHHHHH\n"
        let mut buf = *b"lunarbg: FATAL signal 00 fault-addr 0x0000000000000000\n";
        buf[22] = b'0' + ((sig / 10) % 10) as u8;
        buf[23] = b'0' + (sig % 10) as u8;
        for i in 0..16 {
            let nibble = ((addr >> ((15 - i) * 4)) & 0xf) as u8;
            buf[38 + i] = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + nibble - 10
            };
        }
        unsafe {
            libc::write(2, buf.as_ptr() as *const libc::c_void, buf.len());
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }
    unsafe {
        let mut sa: libc::sigaction = core::mem::zeroed();
        let mut old: libc::sigaction = core::mem::zeroed();
        sa.sa_sigaction = handler as *const () as usize;
        // SA_ONSTACK: the Rust runtime installs its own SIGSEGV handler on an
        // alternate signal stack so a STACK OVERFLOW can still be reported —
        // on an exhausted stack a normal handler cannot run at all. Replacing
        // it without the flag made overflows die silently; with it we keep
        // using the altstack the runtime already set up.
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        libc::sigemptyset(&mut sa.sa_mask);
        // Pass a real oldact buffer rather than null: some kernels (including
        // Eclipse OS's microkernel) unconditionally dereference the third
        // argument, causing a fault-addr 0x18 kernel page fault on null.
        libc::sigaction(libc::SIGSEGV, &sa, &mut old);
        libc::sigaction(libc::SIGBUS, &sa, &mut old);
        libc::sigaction(libc::SIGILL, &sa, &mut old);
    }
}

/// SIGTERM/SIGINT exit the main loop for a clean shutdown; SIGUSR1 toggles
/// the animation. Registered WITHOUT SA_RESTART so poll(2) returns EINTR and
/// the main loop reacts immediately instead of after the current timeout.
fn install_signal_handlers() {
    extern "C" fn on_stop(_sig: libc::c_int) {
        RUNNING.store(false, Ordering::Relaxed);
    }
    extern "C" fn on_usr1(_sig: libc::c_int) {
        TOGGLE_ANIMATE.store(true, Ordering::Relaxed);
    }
    unsafe {
        let mut sa: libc::sigaction = core::mem::zeroed();
        // Never pass a null oldact: Eclipse OS's kernel dereferences the
        // third argument unconditionally and faults on null (that is what
        // took down install_crash_handler with "fault-addr 0x18"). These
        // three calls kept the null form, so lunarbg died right after the
        // initial roundtrip — leaving a black desktop with no wallpaper.
        let mut old: libc::sigaction = core::mem::zeroed();
        sa.sa_sigaction = on_stop as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGTERM, &sa, &mut old);
        libc::sigaction(libc::SIGINT, &sa, &mut old);
        sa.sa_sigaction = on_usr1 as *const () as usize;
        libc::sigaction(libc::SIGUSR1, &sa, &mut old);
    }
}

// ----------------------------------------------------------------- CLI

const USAGE: &str = "\
lunarbg — Eclipse OS animated wallpaper (wlr-layer-shell client)

USAGE:
    lunarbg [OPTIONS]

OPTIONS:
    -f, --fps <N>         Target animation rate, 1..=60 (default 24; env
                          LUNARBG_FPS). The real rate is additionally capped
                          by the compositor via frame callbacks.
    -s, --static          Render one frame and stop animating
                          (env LUNARBG_STATIC=1)
    -a, --aspect <RATIO>  Panel aspect override, e.g. \"16:9\" or \"1.778\".
                          Wins over wl_output.geometry (env LUNARBG_ASPECT)
    -o, --output <NAME>   Only paint the named output; repeat the flag for
                          several (default: every output)
    -q, --quiet           Suppress setup checkpoint messages
        --dump <PATH[:WxH]>  Render one frame offscreen to a raw XRGB8888
                          file and exit; no compositor needed
                          (env LUNARBG_DUMP; default size 1920x1080)
        --dump-ms <MS>    Animation timestamp for --dump (env LUNARBG_DUMP_MS)
        --bench [N]       Time N offscreen frames (default 300) and exit
    -h, --help            Show this help
    -V, --version         Show the version

SIGNALS:
    SIGUSR1               Pause/resume the animation
    SIGTERM, SIGINT       Exit cleanly";

#[derive(Default)]
struct Cli {
    fps: Option<u32>,
    static_: bool,
    aspect: Option<f32>,
    outputs: Vec<String>,
    dump: Option<String>,
    dump_ms: Option<u64>,
    bench: Option<u32>,
    quiet: bool,
}

fn cli_die(msg: &str) -> ! {
    eprintln!("lunarbg: {msg} (see lunarbg --help)");
    std::process::exit(2);
}

fn parse_args() -> Cli {
    let mut cli = Cli::default();
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        // Accept both "--opt value" and "--opt=value".
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f.to_string(), Some(v.to_string())),
            None => (arg, None),
        };
        let value = |args: &mut std::iter::Peekable<_>| -> String {
            inline
                .clone()
                .or_else(|| args.next())
                .unwrap_or_else(|| cli_die(&format!("option {flag} needs a value")))
        };
        match flag.as_str() {
            "-f" | "--fps" => {
                let v = value(&mut args);
                cli.fps = match v.parse() {
                    Ok(f) if (1..=60).contains(&f) => Some(f),
                    _ => cli_die(&format!("invalid --fps '{v}' (expected 1..=60)")),
                };
            }
            "-s" | "--static" | "-q" | "--quiet" | "-h" | "--help" | "-V" | "--version"
                if inline.is_some() =>
            {
                cli_die(&format!("option {flag} takes no value"))
            }
            "-s" | "--static" => cli.static_ = true,
            "-a" | "--aspect" => {
                let v = value(&mut args);
                cli.aspect = Some(
                    scene::parse_aspect(&v)
                        .unwrap_or_else(|| cli_die(&format!("invalid --aspect '{v}'"))),
                );
            }
            "-o" | "--output" => cli.outputs.push(value(&mut args)),
            "-q" | "--quiet" => cli.quiet = true,
            "--dump" => cli.dump = Some(value(&mut args)),
            "--dump-ms" => {
                let v = value(&mut args);
                cli.dump_ms = Some(
                    v.parse()
                        .unwrap_or_else(|_| cli_die(&format!("invalid --dump-ms '{v}'"))),
                );
            }
            "--bench" => {
                // Optional count: "--bench 500", "--bench=500" or bare.
                cli.bench = Some(match inline.clone() {
                    Some(v) => v
                        .parse()
                        .unwrap_or_else(|_| cli_die(&format!("invalid --bench '{v}'"))),
                    None => match args.peek().and_then(|n| n.parse().ok()) {
                        Some(n) => {
                            args.next();
                            n
                        }
                        None => 300,
                    },
                });
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("lunarbg {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => cli_die(&format!("unknown option '{other}'")),
        }
    }
    cli
}

/// `--dump`: render one animation frame offscreen to a raw XRGB8888 file.
fn run_dump(spec: &str, t_ms: u64, aspect: Option<f32>) {
    let (path, w, h) = match spec.rsplit_once(':') {
        Some((p, dims)) if dims.contains('x') => {
            let (w, h) = dims.split_once('x').unwrap();
            // Clamp to 1: a 0-wide render has no rows to chunk and would
            // panic inside the banded base passes.
            (
                p.to_string(),
                w.parse().unwrap_or(1920).max(1),
                h.parse().unwrap_or(1080).max(1),
            )
        }
        _ => (spec.to_string(), 1920, 1080),
    };
    if w > fill_guard::MAX_BUFFER_DIM as usize || h > fill_guard::MAX_BUFFER_DIM as usize {
        eprintln!(
            "lunarbg: dump {w}x{h} past MAX_BUFFER_DIM={}",
            fill_guard::MAX_BUFFER_DIM
        );
        std::process::exit(1);
    }
    if w.saturating_mul(h) > fill_guard::MAX_BUFFER_PIXELS {
        eprintln!(
            "lunarbg: dump {w}x{h} past MAX_BUFFER_PIXELS ({} Mpx)",
            fill_guard::MAX_BUFFER_PIXELS >> 20
        );
        std::process::exit(1);
    }
    // Offscreen: honour only the CLI/env aspect override (no geometry).
    let aspect = aspect.or_else(scene::aspect_from_env);
    let lay = scene::layout(w, h, aspect, 1);
    let Some(base) = scene::render_base(w, h, aspect, 1) else {
        eprintln!("lunarbg: render_base failed for dump {w}x{h}");
        std::process::exit(1);
    };
    let mut frame = base.clone();
    scene::render_frame(&mut frame, w, &base, &lay, t_ms);
    if let Err(e) = std::fs::write(&path, frame) {
        eprintln!("lunarbg: write dump failed: {e}");
        std::process::exit(1);
    }
    eprintln!("lunarbg: dumped {w}x{h} XRGB8888 (t={t_ms}ms) to {path}");
}

/// `--bench N`: time the offscreen render loop, for regression testing the
/// renderer on target hardware without a compositor.
fn run_bench(n: u32) {
    let (w, h) = (1920usize, 1080usize);
    let t0 = Instant::now();
    // Explicit None: do not let LUNARBG_ASPECT skew bench timings.
    let lay = scene::layout(w, h, None, 1);
    let Some(base) = scene::render_base(w, h, None, 1) else {
        eprintln!("lunarbg: render_base failed for bench");
        std::process::exit(1);
    };
    let base_ms = t0.elapsed().as_secs_f64() * 1e3;
    let mut frame = base.clone();
    for t in 0..30u64 {
        scene::render_frame(&mut frame, w, &base, &lay, t * 83); // warm-up
    }
    let n = n.max(1);
    let t0 = Instant::now();
    for t in 0..n as u64 {
        scene::render_frame(&mut frame, w, &base, &lay, 2500 + t * 83);
    }
    let per = t0.elapsed().as_secs_f64() * 1e3 / n as f64;
    println!("lunarbg: base scene {w}x{h}: {base_ms:.1} ms");
    println!(
        "lunarbg: {n} frames: {per:.3} ms/frame ({:.0} fps possible, one core)",
        1000.0 / per
    );
}

fn main() {
    install_crash_handler();
    let cli = parse_args();
    if cli.quiet {
        QUIET.store(true, Ordering::Relaxed);
    }

    if let Some(n) = cli.bench {
        run_bench(n);
        return;
    }
    // Offscreen debug mode, also reachable as LUNARBG_DUMP=/path[:WxH].
    let dump = cli
        .dump
        .clone()
        .or_else(|| std::env::var("LUNARBG_DUMP").ok());
    if let Some(spec) = dump {
        let t_ms = cli
            .dump_ms
            .or_else(|| {
                std::env::var("LUNARBG_DUMP_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
            })
            .unwrap_or(0);
        run_dump(&spec, t_ms, cli.aspect);
        return;
    }

    ckpt!("connecting to compositor");
    let conn = match connect_wayland() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lunarbg: cannot connect to the Wayland compositor: {e}");
            std::process::exit(1);
        }
    };
    ckpt!("connected; creating event queue + registry");
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let display = conn.display();
    display.get_registry(&qh, ());

    let animate = !cli.static_
        && std::env::var("LUNARBG_STATIC").map_or(true, |v| {
            let v = v.trim().to_ascii_lowercase();
            !(v == "1" || v == "true" || v == "yes" || v == "on")
        });
    let mut state = State {
        animate,
        aspect_cli: cli.aspect.or_else(scene::aspect_from_env),
        only_outputs: cli.outputs.clone(),
        ..State::default()
    };
    ckpt!("initial roundtrip");
    if let Err(e) = queue.roundtrip(&mut state) {
        eprintln!("lunarbg: initial roundtrip failed: {e}");
        std::process::exit(1);
    }
    ckpt!(
        "roundtrip done: compositor={} shm={} layer_shell={} outputs={}",
        state.compositor.is_some(),
        state.shm.is_some(),
        state.layer_shell.is_some(),
        state.outputs.len()
    );
    if state.layer_shell.is_none() {
        eprintln!("lunarbg: compositor lacks zwlr_layer_shell_v1");
        std::process::exit(1);
    }
    if state.compositor.is_none() || state.shm.is_none() {
        eprintln!("lunarbg: missing wl_compositor or wl_shm");
        std::process::exit(1);
    }

    // Only now that the blocking setup (connect + roundtrip) is behind us:
    // installed earlier, a SIGTERM/SIGINT during a stuck roundtrip would set
    // the flag but never be looked at, leaving the process unkillable by the
    // very signals meant to stop it. Until this point the default disposition
    // (terminate) applies, exactly as before these handlers existed.
    install_signal_handlers();

    // Timer-paced animation loop; see `tick` for how frame callbacks keep the
    // real rate at or below what the compositor can composite.
    let fps: u32 = cli
        .fps
        .or_else(|| {
            std::env::var("LUNARBG_FPS")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .filter(|f| (1..=60).contains(f))
        .unwrap_or(DEFAULT_FPS);
    let interval = Duration::from_micros(1_000_000 / fps as u64);
    let mut next_tick = Instant::now() + interval;

    ckpt!("entering event loop (target fps={fps})");
    while RUNNING.load(Ordering::Relaxed) {
        if TOGGLE_ANIMATE.swap(false, Ordering::Relaxed) {
            state.animate = !state.animate;
            eprintln!(
                "lunarbg: animation {} (SIGUSR1)",
                if state.animate { "resumed" } else { "paused" }
            );
            if state.animate {
                next_tick = Instant::now();
            }
        }
        state.ensure_surfaces(&qh);
        // A full socket buffer (the compositor is behind on reading) surfaces
        // as a WouldBlock error, and it is RECOVERABLE: the unsent bytes stay
        // queued and go out on the next flush. Treating it as fatal — as this
        // did — killed the wallpaper for a transient hiccup. Only a real
        // connection error ends the process.
        state.flush_blocked = false;
        if let Err(e) = queue.flush() {
            let recoverable = matches!(
                &e,
                wayland_client::backend::WaylandError::Io(io)
                    if io.kind() == std::io::ErrorKind::WouldBlock
            );
            if !recoverable {
                eprintln!("lunarbg: connection lost: {e}");
                break;
            }
            state.flush_blocked = true;
        }

        // Wait for server events OR the next animation tick, whichever first.
        if let Some(guard) = queue.prepare_read() {
            // Round the wait UP to the next whole millisecond: poll(2) has
            // millisecond granularity, so truncating a sub-millisecond
            // remainder asks for a 0 ms (non-blocking) poll and the loop
            // spins until the tick is due — at 24 fps the interval is
            // 41.67 ms, so every frame ended in ~0.7 ms of busy-wait.
            let timeout_ms: i32 = if state.animate {
                let left = next_tick.saturating_duration_since(Instant::now());
                (left.as_micros().div_ceil(1000)).min(1000) as i32
            } else {
                1000
            };
            let mut events = libc::POLLIN;
            if state.flush_blocked {
                events |= libc::POLLOUT;
            }
            let mut pfd = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut pfd, 1, timeout_ms.max(0)) };
            if ready > 0 {
                if let Err(e) = guard.read() {
                    let recoverable = matches!(
                        &e,
                        wayland_client::backend::WaylandError::Io(io)
                            if matches!(
                                io.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                            )
                    );
                    if !recoverable {
                        eprintln!("lunarbg: connection lost (read): {e}");
                        break;
                    }
                }
            } else {
                drop(guard);
                // ready == 0 is the timeout — the normal path to the next
                // tick. ready < 0 is an error: EINTR just means a signal
                // arrived (handled at the top of the loop), but anything else
                // would repeat every iteration and spin the CPU at 100% with
                // no way out, so bail instead of burning a core forever.
                if ready < 0 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() != std::io::ErrorKind::Interrupted {
                        eprintln!("lunarbg: poll failed: {err}");
                        break;
                    }
                }
            }
        } else {
            // prepare_read returned None (events already queued). Do not spin:
            // dispatch below, but sleep a bit if the animation timer is far.
            if state.animate {
                let left = next_tick.saturating_duration_since(Instant::now());
                if !left.is_zero() {
                    std::thread::sleep(left.min(Duration::from_millis(1)));
                }
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if let Err(e) = queue.dispatch_pending(&mut state) {
            eprintln!("lunarbg: protocol error: {e}");
            break;
        }

        // Do not enqueue more frames while the socket cannot accept writes.
        if state.animate && !state.flush_blocked && Instant::now() >= next_tick {
            state.tick_all(&qh);
            next_tick += interval;
            // If we fell behind (system busy), resync rather than bursting.
            let now = Instant::now();
            if next_tick < now {
                next_tick = now + interval;
            }
        }
    }

    // SIGTERM/SIGINT or connection loss: tear the surfaces down cleanly.
    ckpt!("shutting down cleanly");
    let mut to_retire = Vec::new();
    for mut bg in state.backgrounds.drain(..) {
        bg.layer.destroy();
        bg.surface.destroy();
        if let Some(mut frames) = bg.frames.take() {
            to_retire.push(State::take_frames_for_retire(&mut frames));
            drop(frames);
        }
    }
    for (gen, pending, p, l) in to_retire {
        state.retire_map(p, l, gen, pending);
    }
    for rm in state.retired_maps.drain(..) {
        unsafe { libc::munmap(rm.ptr as *mut libc::c_void, rm.len) };
    }
    let _ = queue.flush();
}
