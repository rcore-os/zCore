//! lunarbar — Eclipse OS's native panel stack (its waybar replacement).
//!
//! Why not waybar (or Ironbar/Eww/Riftbar): they are GTK applications. GTK's
//! GApplication registers on the session D-Bus at startup, so on a system with
//! no session bus the panel prints "Could not connect: Connection refused" and
//! exits before it ever maps — and GTK further pulls in gdk-pixbuf, fontconfig
//! and a UTF-8 locale to draw anything. lunarbar instead is a single static
//! musl binary over wlr-layer-shell + wl_shm, with its own bitmap font and
//! /proc readers: NO GTK, NO D-Bus, NO gdk-pixbuf, NO fontconfig, NO locale.
//!
//! Two bars per output, keeping the layout of the waybar config this replaces
//! (rounded pills, 2px accent rule) in Eclipse's midnight palette — a
//! blue-black ground (#060a14) with dark-blue accents (#275a9e rule):
//! - BOTTOM (the classic waybar layout, replicated): ◑ launcher, then one
//!   rounded button per open window via wlr-foreign-toplevel-management;
//!   on the right `cpu N%`, `mem N%`, and the bold clock in its violet pill.
//! - TOP (system info): ☾ eclipse wordmark, uptime and load on the left;
//!   network ▼/▲ throughput, disk, temperature and battery (auto-hidden when
//!   absent) with load-tinted mini gauges, and the date pill on the right.
//!
//! Interaction, matching what a full desktop panel offers:
//! - Hover feedback on every clickable (buttons, launcher, clock pills), with
//!   event-driven repaints — no waiting for the 1 Hz tick — and a proper
//!   arrow/hand pointer via cursor-shape-v1 where the compositor supports it.
//! - Taskbar buttons carry app icons (XDG theme PNGs by app_id, letter badge
//!   fallback — see icons.rs) and shrink to fit instead of dropping windows,
//!   collapsing to icon-only under pressure; left click focuses (or minimizes
//!   the already-active window), middle click closes, the scroll wheel cycles
//!   through windows, and minimized windows draw dimmed. Hovering a truncated
//!   button pops a tooltip with the full title.
//! - The ◑/☾ launcher opens an application menu (icon + name per row) with
//!   search-as-you-type filtering (accent-insensitive), ↑/↓ + Enter keyboard
//!   navigation, and wheel scrolling with a scrollbar when the list overflows.
//! - Clicking the clock or date pill opens a month calendar (today
//!   highlighted, ◂/▸ or wheel to change month, PgUp/PgDn to change year).
//! - cpu/mem ≥ 90%, temperature ≥ 85°C and battery < 15% tint red.
//!
//! Both bars reserve their height as an exclusive zone and repaint at 1 Hz
//! (plus immediate repaints on interaction).
//!
//! Env knobs:
//! - `LUNARBAR_HEIGHT=N`   — bar height in px (default 34, waybar's height).
//! - `LUNARBAR_TERMINAL=…` — command run on launcher click (default
//!   /usr/local/bin/eclipse-terminal).
//! - `LUNARBAR_DUMP=/path:WxH` — render both bars (top + bottom, wallpaper gap
//!   between) to a raw XRGB8888 file and exit, for offline verification.
//!   `LUNARBAR_DUMP_MENU=1` composites the open app menu over the preview;
//!   `LUNARBAR_DUMP_CAL=1` composites the calendar popup instead.

mod apps;
mod fill_guard;
mod icons;
mod par;
mod draw;
mod sysinfo;

use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use draw::{Canvas, Rgb, GLYPH_H, GLYPH_W};
use icons::IconCache;
use sysinfo::{CpuMeter, NetMeter, NetRate};
use wayland_client::{
    protocol::{
        wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_region, wl_registry,
        wl_seat, wl_shm, wl_shm_pool, wl_surface,
    },
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::{self, WpCursorShapeDeviceV1},
    wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

use wp_cursor_shape_device_v1::Shape;

// ── Palette: Eclipse midnight — black ground, dark-blue accents ──────────────
// (Layout still replicates the old waybar config; only the hues changed from
// its violet scheme to near-black + navy.)
const BAR_BG: Rgb = (0x06, 0x0a, 0x14); // bar ground, blue-black
const BAR_RULE: Rgb = (0x27, 0x5a, 0x9e); // 2px border, dark steel blue
const TEXT: Rgb = (0xff, 0xff, 0xff); // primary text, white
const MUTED: Rgb = (0xe8, 0xe8, 0xe8); // module text (cpu/mem/buttons), soft white
const DIM: Rgb = (0x6d, 0x7f, 0xa3); // minimized windows, placeholders
const WARN: Rgb = (0xe0, 0x7a, 0x7a); // hot cpu/temp, low battery (semantic)
const LAUNCH: Rgb = (0x6e, 0xa8, 0xff); // launcher glyph / blue accent
const PILL: Rgb = (0x12, 0x21, 0x38); // clock/date pill background, navy
const PILL_HOVER: Rgb = (0x1a, 0x2f, 0x4e); // pill under the pointer
const BTN_ACTIVE: Rgb = (0x1f, 0x3a, 0x63); // active taskbar button
const WHITE: Rgb = (0xff, 0xff, 0xff); // active button text
const MENU_PANEL: Rgb = (0x0b, 0x12, 0x20); // launcher menu panel, deep navy
const MENU_HOVER: Rgb = (0x1f, 0x3a, 0x63); // hovered menu row

const BUFFERS: usize = 2;

/// Udata marker: distinguishes the popup's layer surface and buffers from the
/// bars' in wayland-client's per-(interface,udata) dispatch.
#[derive(Clone, Copy)]
struct PopupId;

/// Udata marker for the taskbar tooltip's surface and buffers.
#[derive(Clone, Copy)]
struct TipId;

// Keycodes as wl_keyboard.key delivers them: BARE Linux evdev scancodes
// (input-event-codes.h). The famous +8 offset is an xkb keymap convention the
// CLIENT applies when feeding xkbcommon — it is never added on the wire;
// wlroots compositors forward libinput's evdev codes unmodified.
const KEY_ESC_WL: u32 = 1;
const KEY_BACKSPACE_WL: u32 = 14;
const KEY_TAB_WL: u32 = 15;
const KEY_ENTER_WL: u32 = 28;
const KEY_KPENTER_WL: u32 = 96;
const KEY_UP_WL: u32 = 103;
const KEY_DOWN_WL: u32 = 108;
const KEY_LEFT_WL: u32 = 105;
const KEY_RIGHT_WL: u32 = 106;
const KEY_PGUP_WL: u32 = 104;
const KEY_PGDN_WL: u32 = 109;

// Pointer button codes (linux/input-event-codes.h).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

/// One wheel notch in wl_pointer axis units (libinput's convention).
const WHEEL_NOTCH: f64 = 15.0;

/// Hover dwell before the taskbar tooltip appears.
const TIP_DELAY: Duration = Duration::from_millis(450);

/// Post surface damage for a rect in BUFFER pixels. `damage_buffer` needs
/// wl_surface v4; fall back to v1 `damage` in surface coords (÷ scale).
fn damage(surface: &wl_surface::WlSurface, scale: u32, x: i32, y: i32, w: i32, h: i32) {
    if surface.version() >= 4 {
        surface.damage_buffer(x, y, w, h);
        return;
    }
    let s = scale.max(1) as i32;
    let (x0, y0) = (x / s, y / s);
    let (x1, y1) = ((x + w + s - 1) / s, (y + h + s - 1) / s);
    surface.damage(x0, y0, x1 - x0, y1 - y0);
}

/// Everything we track per `wl_output` global.
struct OutputInfo {
    output: wl_output::WlOutput,
    global_name: u32,
    scale: i32,
    /// Last advertised mode size (for configure width/height == 0).
    mode_w: u32,
    mode_h: u32,
}


/// state values from wlr-foreign-toplevel-management-unstable-v1
/// (the `state` array carries u32 enum values).
const TOPLEVEL_STATE_MINIMIZED: u32 = 1;
const TOPLEVEL_STATE_ACTIVATED: u32 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    /// Top bar: system metrics.
    Info,
    /// Bottom bar: waybar-style launcher + taskbar + cpu/mem/clock.
    Task,
}

/// What the pointer is over on a bar — drives hover highlights, the hand
/// cursor and the taskbar tooltip.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hover {
    None,
    Launcher,
    /// A taskbar window button, identified by its foreign-toplevel protocol
    /// id — NOT a list index, so a window opening/closing between the frame
    /// that produced a hitbox and the click that consumes it can never
    /// redirect the action to a neighbouring window.
    Task(u32),
    /// The clock pill (bottom bar) or date pill (top bar).
    Clock,
    /// Volume control module.
    Volume,
    /// Power / Session menu button.
    Power,
}

/// Click/hover target for one taskbar button.
struct TaskHit {
    x0: i32,
    x1: i32,
    /// Foreign-toplevel protocol id of the window this button represents.
    tid: u32,
    /// The shown label lost characters — hovering pops the full-title tooltip.
    truncated: bool,
}

/// A window button's render input, snapshotted from a Toplevel.
struct TaskItem {
    label: String,
    /// Foreign-toplevel protocol id (stable identity for hits/hover).
    tid: u32,
    /// Icon lookup key (Wayland app_id; icons::IconCache follows it to the
    /// theme index or the matching .desktop file's Icon=).
    app_id: String,
    active: bool,
    minimized: bool,
    /// The title was longer than the label cap.
    long: bool,
}

struct Bar {
    role: Role,
    output: wl_output::WlOutput,
    /// Registry name of the output (for GlobalRemove teardown).
    output_global: u32,
    /// Integer HiDPI scale from wl_output.scale (clamped 1..=8).
    scale: u32,
    surface: wl_surface::WlSurface,
    layer: ZwlrLayerSurfaceV1,
    /// Logical (surface) size from layer-shell configure.
    width: u32,
    height: u32,
    map: *mut u8,
    map_len: usize,
    buffers: [Option<wl_buffer::WlBuffer>; BUFFERS],
    busy: [bool; BUFFERS],
    next: usize,
    /// Bumped on every pool rebuild; stale wl_buffer::Release events from the
    /// previous pool must not mark the new buffer at the same slot reusable.
    generation: u64,
    configured: bool,
    /// A render was skipped because the compositor held both buffers; repaint
    /// as soon as one is released instead of waiting for the next tick.
    dirty: bool,
    hover: Hover,
    /// x-range [x0,x1) of the launcher hitbox.
    launcher_hit: (i32, i32),
    /// x-range [x0,x1) of the clock/date pill hitbox (opens the calendar).
    clock_hit: (i32, i32),
    /// x-range [x0,x1) of the volume module hitbox.
    vol_hit: (i32, i32),
    /// x-range [x0,x1) of the power button hitbox.
    power_hit: (i32, i32),
    /// Click targets for each window button (Task bars).
    task_hits: Vec<TaskHit>,
}

impl Drop for Bar {
    fn drop(&mut self) {
        for b in self.buffers.iter().flatten() {
            b.destroy();
        }
        if !self.map.is_null() {
            unsafe { libc::munmap(self.map as *mut libc::c_void, self.map_len) };
        }
        self.layer.destroy();
        self.surface.destroy();
    }
}

/// One entry in the taskbar, mirrored from a foreign-toplevel handle.
struct Toplevel {
    handle: ZwlrForeignToplevelHandleV1,
    title: String,
    app_id: String,
    activated: bool,
    minimized: bool,
}

/// What the launcher/clock/power/volume popup currently shows.
enum PopupKind {
    /// The application menu: full list, live search filter, keyboard
    /// selection and a scroll window over the filtered entries.
    Apps {
        all: Vec<apps::AppEntry>,
        filter: String,
        /// Indices into `all` that match `filter`.
        visible: Vec<usize>,
        /// Selected row (absolute index into `visible`).
        sel: usize,
        /// First visible row of the scroll window.
        scroll: usize,
    },
    /// The month calendar, opened from the clock (bottom) or date (top) pill.
    Calendar { year: i32, month: u32, at_top: bool },
    /// Power / Session menu popup. `at_top` when opened from the top bar, so
    /// the panel hugs the bar that spawned it (the button is on both bars).
    PowerMenu { at_top: bool },
    /// Right-click context menu for one window, keyed by its foreign-toplevel
    /// protocol id (never a list index — the window can close while the menu
    /// is open, and an index would then act on whoever took its slot).
    TaskMenu { tid: u32, title: String, at_top: bool },
    /// Volume control popup slider.
    Volume { level: u32, at_top: bool },
}

/// A clickable region inside the popup panel.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// An app row: (absolute row in the filtered `visible` list, entry index
    /// into `all`). Launching uses the entry index — stable across a
    /// refilter — so a click racing a dirty-skipped repaint still launches
    /// exactly the app whose name was drawn under the cursor.
    Row(usize, usize),
    PrevMonth,
    NextMonth,
    PowerLock,
    PowerLogout,
    PowerReboot,
    PowerShutdown,
    TaskFocus(u32),
    TaskMinimize(u32),
    TaskClose(u32),
    /// Click anywhere on the volume gauge; percent derived from x.
    VolumeGauge,
}

/// What a popup draw pass reports back: the panel rect (x,y,w,h) — clicks
/// outside it dismiss — and the absolute hit rects with their actions.
type PopupFrame = ((i32, i32, i32, i32), Vec<(i32, i32, i32, i32, Action)>);

/// The launcher menu / calendar: a full-output translucent overlay surface
/// (Overlay layer, ARGB) with a clickable panel. Created on demand, torn down
/// when dismissed.
struct Popup {
    surface: wl_surface::WlSurface,
    layer: ZwlrLayerSurfaceV1,
    width: u32,
    height: u32,
    map: *mut u8,
    map_len: usize,
    buffers: [Option<wl_buffer::WlBuffer>; BUFFERS],
    busy: [bool; BUFFERS],
    next: usize,
    /// See [`Bar::generation`].
    generation: u64,
    configured: bool,
    dirty: bool,
    kind: PopupKind,
    /// Absolute hit rects (x0,y0,x1,y1) and what clicking them does.
    hits: Vec<(i32, i32, i32, i32, Action)>,
    /// Panel rect (x,y,w,h) — clicks outside it dismiss the popup.
    panel: (i32, i32, i32, i32),
}

impl Drop for Popup {
    fn drop(&mut self) {
        for b in self.buffers.iter().flatten() {
            b.destroy();
        }
        if !self.map.is_null() {
            unsafe { libc::munmap(self.map as *mut libc::c_void, self.map_len) };
        }
        self.layer.destroy();
        self.surface.destroy();
    }
}

/// The taskbar tooltip: a tiny non-interactive overlay above a hovered button
/// showing the window's full title. Its input region is empty so it never
/// steals pointer focus from the bar beneath it.
struct Tooltip {
    surface: wl_surface::WlSurface,
    layer: ZwlrLayerSurfaceV1,
    width: u32,
    height: u32,
    map: *mut u8,
    map_len: usize,
    buffers: [Option<wl_buffer::WlBuffer>; BUFFERS],
    busy: [bool; BUFFERS],
    next: usize,
    /// See [`Bar::generation`].
    generation: u64,
    text: String,
    /// (bar layer id, foreign-toplevel protocol id) this tooltip belongs to.
    for_bar: u32,
    for_task: u32,
}

impl Drop for Tooltip {
    fn drop(&mut self) {
        for b in self.buffers.iter().flatten() {
            b.destroy();
        }
        if !self.map.is_null() {
            unsafe { libc::munmap(self.map as *mut libc::c_void, self.map_len) };
        }
        self.layer.destroy();
        self.surface.destroy();
    }
}

/// One coherent snapshot of every metric both bars draw. Refreshed once per
/// 1 Hz tick (a single /proc pass; sampling CpuMeter twice per tick would
/// zero its delta).
#[derive(Default, Clone)]
struct Metrics {
    cpu: Option<u32>,
    mem: Option<u32>,
    clock: String,
    date: String,
    net: Option<NetRate>,
    uptime: Option<String>,
    load: Option<f32>,
    disk: Option<u32>,
    temp: Option<u32>,
    batt: Option<(u32, bool)>,
    vol: Option<u32>,
}

#[derive(Default)]
struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
    foreign_mgr: Option<ZwlrForeignToplevelManagerV1>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    cursor_mgr: Option<WpCursorShapeManagerV1>,
    cursor_dev: Option<WpCursorShapeDeviceV1>,
    cursor_current: Option<Shape>,
    /// Bound wl_output globals (capped).
    outputs: Vec<OutputInfo>,
    /// Registry name of the seat we bound (first wins; later seats ignored).
    seat_name: Option<u32>,
    /// Old shm mappings awaiting safe munmap after a resize.
    retired_maps: Vec<(*mut u8, usize)>,
    bars: Vec<Bar>,
    toplevels: Vec<Toplevel>,
    popup: Option<Popup>,
    tooltip: Option<Tooltip>,
    /// A truncated button (bar layer id, toplevel protocol id) is being
    /// hovered; show its tooltip at the deadline.
    tip_pending: Option<(u32, u32, Instant)>,
    height: u32,
    terminal: String,
    cpu: CpuMeter,
    net: NetMeter,
    metrics: Metrics,
    /// Cached mixer level: read once at startup and updated when the user
    /// drags the slider, never re-read per tick. None hides the module.
    vol: Option<u32>,
    /// Cached XDG application list, and whether the scan has run (see
    /// toggle_apps: the walk is far too expensive to repeat per click).
    apps_cache: Vec<apps::AppEntry>,
    apps_scanned: bool,
    icons: IconCache,
    generation: u64,
    /// 1 Hz tick counter (drives the search caret blink).
    tick: u64,
    // pointer tracking for clicks
    ptr_x: f64,
    ptr_y: f64,
    ptr_serial: u32,       // serial of the last pointer Enter (for set_shape)
    ptr_bar: Option<u32>,  // layer id the pointer is over
    ptr_on_popup: bool,    // pointer is over the popup overlay
    scroll_acc: f64,       // accumulated wheel distance until one notch
}

impl State {
    /// One metrics pass per tick, shared by every bar render until the next.
    fn refresh_metrics(&mut self) {
        self.metrics = Metrics {
            cpu: self.cpu.sample(),
            mem: sysinfo::mem_percent(),
            clock: sysinfo::clock_hhmm(),
            date: sysinfo::date_dm(),
            net: self.net.sample(),
            uptime: sysinfo::uptime(),
            load: sysinfo::loadavg(),
            disk: sysinfo::disk_root_percent(),
            temp: sysinfo::temp_c(),
            batt: sysinfo::battery(),
            // Cached: reading the mixer forks a helper, far too costly for
            // the 1 Hz tick. Primed at startup, updated when the user drags
            // the slider.
            vol: self.vol,
        };
    }

    fn ensure_surfaces(&mut self, qh: &QueueHandle<State>) {
        let (Some(compositor), Some(layer_shell)) = (&self.compositor, &self.layer_shell) else {
            return;
        };
        // Outputs that already have bars (by registry name).
        let claimed: std::collections::HashSet<u32> =
            self.bars.iter().map(|b| b.output_global).collect();
        let pending: Vec<(u32, wl_output::WlOutput, u32)> = self
            .outputs
            .iter()
            .filter(|o| !claimed.contains(&o.global_name))
            .map(|o| {
                (
                    o.global_name,
                    o.output.clone(),
                    o.scale.clamp(1, 8) as u32,
                )
            })
            .collect();
        for (global_name, output, scale) in pending {
            for (role, edge) in [(Role::Info, Anchor::Top), (Role::Task, Anchor::Bottom)] {
                if self.bars.len() >= fill_guard::MAX_BARS {
                    eprintln!(
                        "lunarbar: ignoring bar past MAX_BARS={}",
                        fill_guard::MAX_BARS
                    );
                    break;
                }
                let surface = compositor.create_surface(qh, ());
                let layer = layer_shell.get_layer_surface(
                    &surface,
                    Some(&output),
                    zwlr_layer_shell_v1::Layer::Top,
                    "panel".into(),
                    qh,
                    (),
                );
                layer.set_anchor(edge | Anchor::Left | Anchor::Right);
                layer.set_size(0, self.height);
                layer.set_exclusive_zone(self.height as i32);
                layer.set_keyboard_interactivity(KeyboardInteractivity::None);
                surface.commit();
                let bar = Bar {
                    role,
                    output: output.clone(),
                    output_global: global_name,
                    scale,
                    surface,
                    layer,
                    width: 0,
                    height: self.height,
                    map: std::ptr::null_mut(),
                    map_len: 0,
                    buffers: [None, None],
                    busy: [false, false],
                    next: 0,
                    generation: 0,
                    configured: false,
                    dirty: false,
                    hover: Hover::None,
                    launcher_hit: (0, 0),
                    clock_hit: (0, 0),
                    vol_hit: (0, 0),
                    power_hit: (0, 0),
                    task_hits: Vec::new(),
                };
                if !fill_guard::try_push_bounded(&mut self.bars, bar, fill_guard::MAX_BARS) {
                    // unreachable given the check above; keep for symmetry
                }
            }
        }
    }

    fn retire_map(&mut self, map: *mut u8, map_len: usize) {
        if map.is_null() || map_len == 0 {
            return;
        }
        self.retired_maps.push((map, map_len));
        while self.retired_maps.len() > fill_guard::MAX_RETIRED_MAPS {
            let (p, l) = self.retired_maps.remove(0);
            unsafe { libc::munmap(p as *mut libc::c_void, l) };
        }
    }

    fn clear_pointer_hover(&mut self) {
        self.ptr_bar = None;
        self.tip_pending = None;
        self.tooltip = None;
        let mut dirty = Vec::new();
        for bar in &mut self.bars {
            if bar.hover != Hover::None {
                bar.hover = Hover::None;
                dirty.push(bar.layer.id().protocol_id());
            }
        }
        for id in dirty {
            self.render(id);
        }
    }

    fn output_scale(&self, global: u32) -> u32 {
        self.outputs
            .iter()
            .find(|o| o.global_name == global)
            .map(|o| o.scale.clamp(1, 8) as u32)
            .unwrap_or(1)
    }

    fn output_mode(&self, global: u32) -> (u32, u32) {
        self.outputs
            .iter()
            .find(|o| o.global_name == global)
            .map(|o| (o.mode_w, o.mode_h))
            .unwrap_or((0, 0))
    }

    /// Allocate a fresh shm mapping + fd. Caller installs buffers. On failure
    /// returns None without touching existing bar state.
    fn map_shm_pool(total: usize) -> Option<(*mut u8, OwnedFd)> {
        let raw = unsafe {
            libc::memfd_create(b"lunarbar\0".as_ptr() as *const libc::c_char, libc::MFD_CLOEXEC)
        };
        if raw < 0 {
            eprintln!("lunarbar: memfd_create failed");
            return None;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        if unsafe { libc::ftruncate(raw, total as libc::off_t) } != 0 {
            eprintln!("lunarbar: ftruncate failed");
            return None;
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
            eprintln!("lunarbar: mmap failed");
            return None;
        }
        Some((map as *mut u8, fd))
    }

    fn bar_index(&self, layer_id: u32) -> Option<usize> {
        self.bars
            .iter()
            .position(|b| b.layer.id().protocol_id() == layer_id)
    }

    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.generation
    }

    /// (Re)allocate the shm pool for a bar after a configure with a new size.
    fn configure(&mut self, qh: &QueueHandle<State>, layer_id: u32, mut w: u32, mut h: u32) {
        let Some(idx) = self.bar_index(layer_id) else {
            return;
        };
        let global = self.bars[idx].output_global;
        let scale = self.output_scale(global);
        self.bars[idx].scale = scale;
        // Protocol: 0 means "client decides" — use the last mode size.
        if w == 0 || h == 0 {
            let (mw, mh) = self.output_mode(global);
            if w == 0 {
                w = mw.max(1);
            }
            if h == 0 {
                h = self.height.max(1).min(mh.max(self.height));
            }
        }
        w = w.max(1);
        h = h.max(1);
        if self.bars[idx].configured && self.bars[idx].width == w && self.bars[idx].height == h {
            self.render(layer_id);
            return;
        }
        let Some(shm) = self.shm.clone() else { return };

        let bw = w.saturating_mul(scale);
        let bh = h.saturating_mul(scale);
        if bw > fill_guard::MAX_BUFFER_DIM || bh > fill_guard::MAX_BUFFER_DIM {
            eprintln!("lunarbar: {bw}x{bh} bar past MAX_BUFFER_DIM; skipping");
            return;
        }
        if (bw as usize).saturating_mul(bh as usize) > fill_guard::MAX_BUFFER_PIXELS {
            eprintln!("lunarbar: {bw}x{bh} bar past MAX_BUFFER_PIXELS; skipping");
            return;
        }
        let Some(total) = (bw as usize)
            .checked_mul(4)
            .and_then(|s| s.checked_mul(bh as usize))
            .and_then(|f| f.checked_mul(BUFFERS))
            .filter(|t| *t <= i32::MAX as usize)
        else {
            eprintln!("lunarbar: {bw}x{bh} bar too large for wl_shm; skipping");
            return;
        };
        let stride = bw as usize * 4;
        let frame_size = stride * bh as usize;

        // Allocate the NEW pool first. If this fails we keep the previous
        // mapping/buffers so the bar stays mapped and exclusive.
        let Some((map, fd)) = Self::map_shm_pool(total) else {
            return;
        };

        let generation = self.next_generation();
        let pool = shm.create_pool(fd.as_fd(), total as i32, qh, ());
        let mk = |i: usize| {
            pool.create_buffer(
                (i * frame_size) as i32,
                bw as i32,
                bh as i32,
                stride as i32,
                wl_shm::Format::Xrgb8888,
                qh,
                (layer_id, i, generation),
            )
        };
        let buffers = [Some(mk(0)), Some(mk(1))];
        pool.destroy();

        // Swap in the new pool; retire the old mapping instead of munmap'ing
        // while the compositor may still hold the old buffers.
        let (old_map, old_len) = {
            let bar = &mut self.bars[idx];
            for b in bar.buffers.iter_mut() {
                if let Some(b) = b.take() {
                    b.destroy();
                }
            }
            let old = (bar.map, bar.map_len);
            bar.task_hits.clear();
            bar.launcher_hit = (0, 0);
            bar.clock_hit = (0, 0);
            bar.vol_hit = (0, 0);
            bar.power_hit = (0, 0);
            bar.hover = Hover::None;
            bar.width = w;
            bar.height = h;
            bar.scale = scale;
            bar.map = map;
            bar.map_len = total;
            bar.buffers = buffers;
            bar.busy = [false, false];
            bar.next = 0;
            bar.generation = generation;
            bar.configured = true;
            // Exclusive zone tracks the *configured* height, not the request.
            bar.layer.set_exclusive_zone(h as i32);
            if bar.surface.version() >= 3 {
                bar.surface.set_buffer_scale(scale as i32);
            }
            old
        };
        self.retire_map(old_map, old_len);
        self.render(layer_id);
    }

    fn render(&mut self, layer_id: u32) {
        let Some(idx) = self.bar_index(layer_id) else {
            return;
        };
        if !self.bars[idx].configured || self.bars[idx].map.is_null() {
            return;
        }
        let m = self.metrics.clone();
        match self.bars[idx].role {
            Role::Info => {
                let (w, h) = (self.bars[idx].width as usize, self.bars[idx].height as usize);
                let scale = self.bars[idx].scale.max(1);
                let frame_size = w * scale as usize * h * scale as usize * 4;
                let hover = self.bars[idx].hover;
                let bar = &mut self.bars[idx];
                let Some(i) = pick_buffer(bar) else {
                    bar.dirty = true; // repaint on the next buffer Release
                    return;
                };
                let Some(mut cv) = Canvas::try_new(w, h) else {
                    bar.busy[i] = false;
                    return;
                };
                let (launcher_hit, clock_hit, power_hit) = draw_info(&mut cv, w, h, &m, hover);
                let bar = &mut self.bars[idx];
                let data: &mut [u8] = unsafe {
                    std::slice::from_raw_parts_mut(bar.map.add(i * frame_size), frame_size)
                };
                if !cv.blit_xrgb_scaled(data, scale) {
                    bar.busy[i] = false;
                    return;
                }
                bar.launcher_hit = launcher_hit;
                bar.clock_hit = clock_hit;
                bar.power_hit = power_hit;
                commit_bar(bar, i);
            }
            Role::Task => {
                let items: Vec<TaskItem> = self
                    .toplevels
                    .iter()
                    .map(|t| {
                        let (label, long) = button_label(t);
                        TaskItem {
                            label,
                            tid: t.handle.id().protocol_id(),
                            app_id: t.app_id.clone(),
                            active: t.activated,
                            minimized: t.minimized,
                            long,
                        }
                    })
                    .collect();
                let (w, h) = (self.bars[idx].width as usize, self.bars[idx].height as usize);
                let scale = self.bars[idx].scale.max(1);
                let frame_size = w * scale as usize * h * scale as usize * 4;
                let hover = self.bars[idx].hover;
                let bar = &mut self.bars[idx];
                let Some(i) = pick_buffer(bar) else {
                    bar.dirty = true;
                    return;
                };
                let Some(mut cv) = Canvas::try_new(w, h) else {
                    bar.busy[i] = false;
                    return;
                };
                let (launcher_hit, hits, clock_hit, vol_hit, power_hit) =
                    draw_task(&mut cv, w, h, &items, &m, hover, &mut self.icons);
                let bar = &mut self.bars[idx];
                let data: &mut [u8] = unsafe {
                    std::slice::from_raw_parts_mut(bar.map.add(i * frame_size), frame_size)
                };
                if !cv.blit_xrgb_scaled(data, scale) {
                    bar.busy[i] = false;
                    return;
                }
                bar.launcher_hit = launcher_hit;
                bar.clock_hit = clock_hit;
                bar.vol_hit = vol_hit;
                bar.power_hit = power_hit;
                bar.task_hits = hits;
                commit_bar(bar, i);
            }
        }
    }

    fn render_all(&mut self) {
        let ids: Vec<u32> = self
            .bars
            .iter()
            .filter(|b| b.configured)
            .map(|b| b.layer.id().protocol_id())
            .collect();
        for id in ids {
            self.render(id);
        }
    }

    fn render_task_bars(&mut self) {
        let ids: Vec<u32> = self
            .bars
            .iter()
            .filter(|b| b.configured && b.role == Role::Task)
            .map(|b| b.layer.id().protocol_id())
            .collect();
        for id in ids {
            self.render(id);
        }
    }

    /// Run a shell command detached via double-fork. The intermediate child
    /// exits immediately (parent reaps it with a blocking waitpid, which
    /// returns at once), so the grandchild is reparented to init — lunarbar
    /// never accumulates zombies.
    fn spawn(&self, cmd: &str) {
        // Build the argv string BEFORE forking. Between fork() and exec() a
        // child may call only async-signal-safe functions, and CString::new
        // allocates. Today that is latent rather than live — par_rows joins
        // its workers before returning, so no thread is alive when the event
        // loop calls this — but the safety rests on an invariant nothing
        // enforces: add any background thread (an async icon loader, say) and
        // a fork landing while it held the allocator lock would leave the
        // child deadlocked forever on a lock whose owner does not exist in it,
        // as a launch that silently never happens. Cheap to make structural.
        let Ok(c) = std::ffi::CString::new(cmd) else {
            return; // an interior NUL cannot be passed to exec at all
        };
        unsafe {
            let pid = libc::fork();
            if pid == 0 {
                // Intermediate child: new session, fork the real child, exit.
                libc::setsid();
                if libc::fork() == 0 {
                    let sh = b"/bin/sh\0";
                    let dashc = b"-c\0";
                    let argv = [
                        sh.as_ptr() as *const libc::c_char,
                        dashc.as_ptr() as *const libc::c_char,
                        c.as_ptr(),
                        std::ptr::null(),
                    ];
                    libc::execv(sh.as_ptr() as *const libc::c_char, argv.as_ptr());
                    libc::_exit(127);
                }
                libc::_exit(0);
            }
            if pid > 0 {
                let mut st = 0;
                libc::waitpid(pid, &mut st, 0);
            }
        }
    }

    // ── Taskbar interaction ─────────────────────────────────────────────────

    /// A click on a window button (by toplevel protocol id): left focuses
    /// (or minimizes the window that is already active — the classic taskbar
    /// toggle), middle closes.
    fn task_click(&self, tid: u32, button: u32) {
        let Some(t) = self
            .toplevels
            .iter()
            .find(|t| t.handle.id().protocol_id() == tid)
        else {
            return;
        };
        match button {
            BTN_MIDDLE => t.handle.close(),
            BTN_LEFT => {
                if t.activated {
                    t.handle.set_minimized();
                } else {
                    if t.minimized {
                        t.handle.unset_minimized();
                    }
                    if let Some(seat) = &self.seat {
                        t.handle.activate(seat);
                    }
                }
            }
            _ => {}
        }
    }

    /// Wheel over the taskbar: cycle focus through the window list.
    fn cycle_windows(&self, dir: i32) {
        let n = self.toplevels.len();
        if n == 0 {
            return;
        }
        let next = match self.toplevels.iter().position(|t| t.activated) {
            Some(c) => (c as i32 + dir).rem_euclid(n as i32) as usize,
            None if dir > 0 => 0,
            None => n - 1,
        };
        let Some(seat) = &self.seat else { return };
        let t = &self.toplevels[next];
        if t.minimized {
            t.handle.unset_minimized();
        }
        t.handle.activate(seat);
    }

    // ── Popup (app menu / calendar) ─────────────────────────────────────────

    /// Launcher click: toggle the application menu.
    fn toggle_apps(&mut self, qh: &QueueHandle<State>) {
        if matches!(&self.popup, Some(p) if matches!(p.kind, PopupKind::Apps { .. })) {
            self.close_popup();
            return;
        }
        // Scan the XDG application directories once and reuse the result:
        // re-walking them on every launcher click means thousands of path
        // lookups per click, for a list that changes only when software is
        // installed. `apps_scanned` is cleared by nothing today — a session
        // that installs an app can reopen the panel to pick it up.
        if !self.apps_scanned {
            self.apps_cache = vec![apps::AppEntry {
                name: "Terminal".into(),
                exec: self.terminal.clone(),
                icon: Some("utilities-terminal".into()),
            }];
            self.apps_cache.extend(apps::scan_apps(&self.terminal));
            self.apps_scanned = true;
        }
        let all: Vec<apps::AppEntry> = self
            .apps_cache
            .iter()
            .map(|e| apps::AppEntry {
                name: e.name.clone(),
                exec: e.exec.clone(),
                icon: e.icon.clone(),
            })
            .collect();
        let visible = (0..all.len()).collect();
        self.open_popup(
            qh,
            PopupKind::Apps {
                all,
                filter: String::new(),
                visible,
                sel: 0,
                scroll: 0,
            },
        );
    }

    /// Clock/date pill click: toggle the month calendar.
    fn toggle_calendar(&mut self, qh: &QueueHandle<State>, at_top: bool) {
        if matches!(&self.popup, Some(p) if matches!(p.kind, PopupKind::Calendar { .. })) {
            self.close_popup();
            return;
        }
        let Some((year, month, _)) = sysinfo::today() else {
            return;
        };
        self.open_popup(qh, PopupKind::Calendar { year, month, at_top });
    }

    /// Power button click: toggle the session power menu.
    fn toggle_power(&mut self, qh: &QueueHandle<State>, at_top: bool) {
        if matches!(&self.popup, Some(p) if matches!(p.kind, PopupKind::PowerMenu { .. })) {
            self.close_popup();
            return;
        }
        self.open_popup(qh, PopupKind::PowerMenu { at_top });
    }

    /// Right click on window button: toggle task context menu.
    fn toggle_task_menu(&mut self, qh: &QueueHandle<State>, tid: u32) {
        if matches!(&self.popup, Some(p) if matches!(p.kind, PopupKind::TaskMenu { tid: open_tid, .. } if open_tid == tid)) {
            self.close_popup();
            return;
        }
        let title = self
            .toplevels
            .iter()
            .find(|t| t.handle.id().protocol_id() == tid)
            .map(|t| t.title.clone())
            .unwrap_or_default();
        self.open_popup(
            qh,
            PopupKind::TaskMenu {
                tid,
                title,
                at_top: false,
            },
        );
    }

    /// Volume module click: toggle volume slider popup.
    fn toggle_volume(&mut self, qh: &QueueHandle<State>, at_top: bool) {
        if matches!(&self.popup, Some(p) if matches!(p.kind, PopupKind::Volume { .. })) {
            self.close_popup();
            return;
        }
        let level = self.vol.unwrap_or(0);
        self.open_popup(qh, PopupKind::Volume { level, at_top });
    }

    /// Map a full-output overlay surface for `kind`, replacing any open popup.
    fn open_popup(&mut self, qh: &QueueHandle<State>, kind: PopupKind) {
        let (Some(comp), Some(ls)) = (&self.compositor, &self.layer_shell) else {
            return;
        };
        self.tooltip = None;
        self.tip_pending = None;
        self.popup = None; // Drop tears down any previous overlay first

        // Map on the output whose bar was clicked. Passing None would let the
        // compositor choose, so on a multi-monitor desk the menu (and the
        // scrim that catches its dismiss-click) could land on another screen
        // than the button that opened it.
        let output = self
            .ptr_bar
            .and_then(|id| self.bar_index(id))
            .map(|i| self.bars[i].output.clone());

        let surface = comp.create_surface(qh, ());
        let layer = ls.get_layer_surface(
            &surface,
            output.as_ref(),
            zwlr_layer_shell_v1::Layer::Overlay,
            "menu".into(),
            qh,
            PopupId,
        );
        layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
        layer.set_size(0, 0);
        // -1: span the FULL output. With the default zone 0 the compositor
        // would size this surface to the usable area (already minus both
        // bars' exclusive zones) and every bar_h offset in the popup math
        // would double-subtract — and the scrim would not cover the bars, so
        // taskbar clicks would land on windows while we hold the keyboard.
        layer.set_exclusive_zone(-1);
        // Exclusive: grab the keyboard while the popup is up, so search typing
        // and arrow navigation work no matter where focus was before.
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        surface.commit();
        self.popup = Some(Popup {
            surface,
            layer,
            width: 0,
            height: 0,
            map: std::ptr::null_mut(),
            map_len: 0,
            buffers: [None, None],
            busy: [false, false],
            next: 0,
            generation: 0,
            configured: false,
            dirty: false,
            kind,
            hits: Vec::new(),
            panel: (0, 0, 0, 0),
        });
    }

    /// Tear down the popup overlay (Drop destroys its surfaces and mapping).
    fn close_popup(&mut self) {
        self.popup = None;
        self.ptr_on_popup = false;
    }

    /// (Re)allocate the popup overlay's ARGB shm pool after a configure.
    fn configure_popup(&mut self, qh: &QueueHandle<State>, mut w: u32, mut h: u32) {
        let Some(shm) = self.shm.clone() else {
            return;
        };
        if w == 0 || h == 0 {
            // Client decides — use the largest known mode as a fallback.
            let (mw, mh) = self
                .outputs
                .iter()
                .map(|o| (o.mode_w, o.mode_h))
                .max_by_key(|(a, b)| (*a as u64) * (*b as u64))
                .unwrap_or((1280, 720));
            if w == 0 {
                w = mw.max(1);
            }
            if h == 0 {
                h = mh.max(1);
            }
        }
        w = w.max(1);
        h = h.max(1);
        if matches!(self.popup.as_ref(), Some(popup) if popup.configured && popup.width == w && popup.height == h) {
            self.render_popup();
            return;
        }
        if w > fill_guard::MAX_BUFFER_DIM || h > fill_guard::MAX_BUFFER_DIM {
            eprintln!("lunarbar: popup {w}x{h} past MAX_BUFFER_DIM; skipping");
            return;
        }
        if (w as usize).saturating_mul(h as usize) > fill_guard::MAX_BUFFER_PIXELS {
            eprintln!("lunarbar: popup {w}x{h} past MAX_BUFFER_PIXELS; skipping");
            return;
        }
        let Some(total) = (w as usize)
            .checked_mul(4)
            .and_then(|s| s.checked_mul(h as usize))
            .and_then(|f| f.checked_mul(BUFFERS))
            .filter(|t| *t <= i32::MAX as usize)
        else {
            return;
        };
        let stride = w as usize * 4;
        let frame_size = stride * h as usize;
        let Some((map, fd)) = Self::map_shm_pool(total) else {
            return;
        };
        let generation = self.next_generation();
        let pool = shm.create_pool(fd.as_fd(), total as i32, qh, ());
        let mk = |i: usize| {
            pool.create_buffer(
                (i * frame_size) as i32,
                w as i32,
                h as i32,
                stride as i32,
                wl_shm::Format::Argb8888,
                qh,
                (PopupId, i, generation),
            )
        };
        let buffers = [Some(mk(0)), Some(mk(1))];
        pool.destroy();

        let (old_map, old_len) = {
            let Some(popup) = self.popup.as_mut() else {
                unsafe { libc::munmap(map as *mut libc::c_void, total) };
                return;
            };
            for b in popup.buffers.iter_mut() {
                if let Some(b) = b.take() {
                    b.destroy();
                }
            }
            let old = (popup.map, popup.map_len);
            popup.width = w;
            popup.height = h;
            popup.map = map;
            popup.map_len = total;
            popup.buffers = buffers;
            popup.busy = [false, false];
            popup.next = 0;
            popup.generation = generation;
            popup.configured = true;
            old
        };
        self.retire_map(old_map, old_len);
        self.render_popup();
    }

    /// Paint the popup overlay into a free buffer and commit it.
    fn render_popup(&mut self) {
        let bar_h = self.height as i32;
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        if !popup.configured || popup.map.is_null() {
            return;
        }
        let (w, h) = (popup.width as usize, popup.height as usize);
        let frame_size = w * h * 4;
        // Pick a released buffer or defer to the next Release.
        let i = if !popup.busy[popup.next] {
            popup.next
        } else if !popup.busy[1 - popup.next] {
            1 - popup.next
        } else {
            popup.dirty = true;
            return;
        };
        popup.next = 1 - i;
        popup.busy[i] = true;

        let Some(mut cv) = Canvas::try_new(w, h) else {
            popup.busy[i] = false;
            return;
        };
        let (panel, hits) = match &mut popup.kind {
            PopupKind::Apps {
                all,
                filter,
                visible,
                sel,
                scroll,
            } => {
                // Clamp the scroll window and selection against the current
                // layout before painting (the output size may have changed).
                let rows_fit = apps_rows_fit(h as i32, bar_h);
                if !visible.is_empty() && *sel >= visible.len() {
                    *sel = visible.len() - 1;
                }
                let max_scroll = visible.len().saturating_sub(rows_fit);
                if *scroll > max_scroll {
                    *scroll = max_scroll;
                }
                draw_apps(
                    &mut cv, w, h, bar_h, all, visible, filter, *sel, *scroll,
                    &mut self.icons,
                )
            }
            PopupKind::Calendar { year, month, at_top } => {
                draw_calendar(&mut cv, w, h, bar_h, *year, *month, *at_top)
            }
            PopupKind::PowerMenu { at_top } => draw_power_menu(&mut cv, w, h, bar_h, *at_top),
            PopupKind::TaskMenu { tid, title, at_top } => {
                draw_task_menu(&mut cv, w, h, bar_h, *tid, title, *at_top)
            }
            PopupKind::Volume { level, at_top } => {
                draw_volume_menu(&mut cv, w, h, bar_h, *level, *at_top)
            }
        };
        let data: &mut [u8] =
            unsafe { std::slice::from_raw_parts_mut(popup.map.add(i * frame_size), frame_size) };
        if !cv.blit_argb(data) {
            popup.busy[i] = false;
            return;
        }
        popup.panel = panel;
        popup.hits = hits;

        if let Some(buf) = popup.buffers[i].as_ref() {
            popup.surface.attach(Some(buf), 0, 0);
            // Damage only the panel (+ small pad) when possible — full-screen
            // damage on every hover was a redraw storm under QEMU resize.
            let (px, py, pw, ph) = panel;
            let pad = 8;
            let dx = (px - pad).max(0);
            let dy = (py - pad).max(0);
            let dw = (pw + pad * 2).min(w as i32 - dx).max(1);
            let dh = (ph + pad * 2).min(h as i32 - dy).max(1);
            damage(&popup.surface, 1, dx, dy, dw, dh);
            // Always include the scrim once on first paint by also damaging
            // the full surface when this is buffer 0 after configure — cheap
            // enough once; hover path uses the panel rect above.
            if i == 0 && popup.busy.iter().filter(|b| **b).count() == 1 {
                damage(&popup.surface, 1, 0, 0, w as i32, h as i32);
            }
            popup.surface.commit();
        }
    }

    /// The popup hit under (x,y), if any.
    fn popup_hit(&self, x: i32, y: i32) -> Option<Action> {
        self.popup.as_ref().and_then(|p| {
            p.hits
                .iter()
                .find(|(x0, y0, x1, y1, _)| x >= *x0 && x < *x1 && y >= *y0 && y < *y1)
                .map(|&(_, _, _, _, a)| a)
        })
    }

    /// Handle a pointer click on the popup overlay.
    fn popup_click(&mut self, x: i32, y: i32) {
        let Some(popup) = self.popup.as_ref() else {
            return;
        };
        let panel = popup.panel;
        match self.popup_hit(x, y) {
            Some(Action::Row(_, entry)) => {
                if let PopupKind::Apps { all, .. } = &popup.kind {
                    if let Some(e) = all.get(entry) {
                        let cmd = e.exec.clone();
                        self.close_popup();
                        self.spawn(&cmd);
                    }
                }
            }
            Some(Action::PrevMonth) => self.cal_shift(-1),
            Some(Action::NextMonth) => self.cal_shift(1),
            Some(Action::PowerLock) => {
                self.close_popup();
                self.spawn("eclipse-lock || swaylock || lock");
            }
            Some(Action::PowerLogout) => {
                self.close_popup();
                self.spawn("labwc --exit || killall labwc || pkill labwc");
            }
            Some(Action::PowerReboot) => {
                self.close_popup();
                self.spawn("reboot || shutdown -r now");
            }
            Some(Action::PowerShutdown) => {
                self.close_popup();
                self.spawn("poweroff || shutdown -h now");
            }
            Some(Action::TaskFocus(tid)) => {
                self.close_popup();
                if let Some(t) = self
                    .toplevels
                    .iter()
                    .find(|t| t.handle.id().protocol_id() == tid)
                {
                    if t.minimized {
                        t.handle.unset_minimized();
                    }
                    if let Some(seat) = &self.seat {
                        t.handle.activate(seat);
                    }
                }
            }
            Some(Action::TaskMinimize(tid)) => {
                self.close_popup();
                if let Some(t) = self
                    .toplevels
                    .iter()
                    .find(|t| t.handle.id().protocol_id() == tid)
                {
                    t.handle.set_minimized();
                }
            }
            Some(Action::TaskClose(tid)) => {
                self.close_popup();
                if let Some(t) = self
                    .toplevels
                    .iter()
                    .find(|t| t.handle.id().protocol_id() == tid)
                {
                    t.handle.close();
                }
            }
            Some(Action::VolumeGauge) => {
                let (px, _py, pw, _ph) = panel;
                let gw = (pw - 30).max(1);
                let rel = (x - (px + 15)).clamp(0, gw);
                let v = ((rel as u32 * 100) / gw as u32).min(100);
                self.vol = Some(v);
                if let Some(popup) = self.popup.as_mut() {
                    if let PopupKind::Volume { level, .. } = &mut popup.kind {
                        *level = v;
                    }
                }
                self.spawn(&format!(
                    "amixer set Master {v}% >/dev/null 2>&1 || wpctl set-volume @DEFAULT_AUDIO_SINK@ {v}%"
                ));
                self.render_popup();
                self.render_all();
            }
            None => {
                let (px, py, pw, ph) = panel;
                if !(x >= px && x < px + pw && y >= py && y < py + ph) {
                    self.close_popup();
                }
            }
        }
    }

    /// Pointer motion over the popup: track the highlighted app row and keep
    /// the cursor shape in sync (hand over clickables).
    fn popup_motion(&mut self, x: i32, y: i32) {
        let hit = self.popup_hit(x, y);
        let mut changed = false;
        if let Some(popup) = self.popup.as_mut() {
            if let (PopupKind::Apps { sel, .. }, Some(Action::Row(r, _))) = (&mut popup.kind, hit) {
                if *sel != r {
                    *sel = r;
                    changed = true;
                }
            }
        }
        if changed {
            self.render_popup();
        }
        self.set_cursor(if hit.is_some() { Shape::Pointer } else { Shape::Default });
    }

    /// Move the calendar by `dir` months (wrapping the year).
    fn cal_shift(&mut self, dir: i32) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let PopupKind::Calendar { year, month, .. } = &mut popup.kind else {
            return;
        };
        let m = *month as i32 + dir;
        *year += m.div_euclid(12);
        *month = m.rem_euclid(12) as u32;
        self.render_popup();
    }

    /// One wheel notch over the popup: scroll the app list / shift the month.
    fn popup_scroll(&mut self, dir: i32) {
        let bar_h = self.height as i32;
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        match &mut popup.kind {
            PopupKind::Apps { visible, scroll, .. } => {
                let rows_fit = apps_rows_fit(popup.height.max(1) as i32, bar_h);
                let max_scroll = visible.len().saturating_sub(rows_fit);
                let new = (*scroll as i32 + dir).clamp(0, max_scroll as i32) as usize;
                if new != *scroll {
                    *scroll = new;
                    self.render_popup();
                }
            }
            PopupKind::Calendar { .. } => self.cal_shift(dir),
            _ => {}
        }
    }

    /// A key press while the popup holds the keyboard.
    fn popup_key(&mut self, key: u32) {
        enum Do {
            Nothing,
            Close,
            Launch(String),
            Cal(i32),
            Render,
        }
        let bar_h = self.height as i32;
        let mut act = Do::Nothing;
        if let Some(popup) = self.popup.as_mut() {
            match &mut popup.kind {
                PopupKind::Apps {
                    all,
                    filter,
                    visible,
                    sel,
                    scroll,
                } => {
                    let rows_fit = apps_rows_fit(popup.height.max(1) as i32, bar_h);
                    match key {
                        KEY_ESC_WL if !filter.is_empty() => {
                            filter.clear();
                            *visible = filter_apps(all, filter);
                            *sel = 0;
                            *scroll = 0;
                            act = Do::Render;
                        }
                        KEY_ESC_WL => act = Do::Close,
                        KEY_ENTER_WL | KEY_KPENTER_WL => {
                            if let Some(&ai) = visible.get(*sel) {
                                act = Do::Launch(all[ai].exec.clone());
                            }
                        }
                        KEY_UP_WL => {
                            *sel = sel.saturating_sub(1);
                            act = Do::Render;
                        }
                        KEY_DOWN_WL | KEY_TAB_WL => {
                            if *sel + 1 < visible.len() {
                                *sel += 1;
                            }
                            act = Do::Render;
                        }
                        KEY_PGUP_WL => {
                            *sel = sel.saturating_sub(rows_fit);
                            act = Do::Render;
                        }
                        KEY_PGDN_WL => {
                            *sel = (*sel + rows_fit).min(visible.len().saturating_sub(1));
                            act = Do::Render;
                        }
                        KEY_BACKSPACE_WL => {
                            if filter.pop().is_some() {
                                *visible = filter_apps(all, filter);
                                *sel = 0;
                                *scroll = 0;
                                act = Do::Render;
                            }
                        }
                        k => {
                            if let Some(c) = key_char(k) {
                                if filter.chars().count() < APPS_FILTER_MAX {
                                    filter.push(c);
                                    *visible = filter_apps(all, filter);
                                    *sel = 0;
                                    *scroll = 0;
                                    act = Do::Render;
                                }
                            }
                        }
                    }
                    if *sel < *scroll {
                        *scroll = *sel;
                    } else if *sel >= *scroll + rows_fit {
                        *scroll = *sel + 1 - rows_fit;
                    }
                }
                PopupKind::Calendar { .. } => match key {
                    KEY_ESC_WL => act = Do::Close,
                    KEY_LEFT_WL | KEY_UP_WL => act = Do::Cal(-1),
                    KEY_RIGHT_WL | KEY_DOWN_WL => act = Do::Cal(1),
                    KEY_PGUP_WL => act = Do::Cal(-12),
                    KEY_PGDN_WL => act = Do::Cal(12),
                    _ => {}
                },
                _ => match key {
                    KEY_ESC_WL => act = Do::Close,
                    _ => {}
                },
            }
        }
        match act {
            Do::Nothing => {}
            Do::Close => self.close_popup(),
            Do::Launch(cmd) => {
                self.close_popup();
                self.spawn(&cmd);
            }
            Do::Cal(d) => self.cal_shift(d),
            Do::Render => self.render_popup(),
        }
    }

    // ── Hover / cursor / tooltip ────────────────────────────────────────────

    /// Pointer moved to `x` on bar `id`: update the hover state, repaint the
    /// bar immediately when it changed, and arm/disarm the tooltip.
    fn bar_motion(&mut self, id: u32, x: i32) {
        let Some(idx) = self.bar_index(id) else {
            return;
        };
        let bar = &self.bars[idx];
        let hover = if x >= bar.launcher_hit.0 && x < bar.launcher_hit.1 {
            Hover::Launcher
        } else if x >= bar.clock_hit.0 && x < bar.clock_hit.1 {
            Hover::Clock
        } else if x >= bar.vol_hit.0 && x < bar.vol_hit.1 {
            Hover::Volume
        } else if x >= bar.power_hit.0 && x < bar.power_hit.1 {
            Hover::Power
        } else if let Some(h) = bar.task_hits.iter().find(|h| x >= h.x0 && x < h.x1) {
            Hover::Task(h.tid)
        } else {
            Hover::None
        };
        if hover != self.bars[idx].hover {
            self.bars[idx].hover = hover;
            // Arm the tooltip only for truncated buttons; anything else
            // dismisses a shown tooltip.
            match hover {
                Hover::Task(tid)
                    if self.bars[idx]
                        .task_hits
                        .iter()
                        .any(|h| h.tid == tid && h.truncated) =>
                {
                    let same = self
                        .tooltip
                        .as_ref()
                        .map(|t| (t.for_bar, t.for_task) == (id, tid))
                        .unwrap_or(false);
                    if !same {
                        self.tooltip = None;
                        self.tip_pending = Some((id, tid, Instant::now()));
                    }
                }
                _ => {
                    self.tooltip = None;
                    self.tip_pending = None;
                }
            }
            self.render(id);
        }
        self.set_cursor(if self.bars[idx].hover == Hover::None {
            Shape::Default
        } else {
            Shape::Pointer
        });
    }

    /// Create the cursor-shape device for the current pointer, if the
    /// compositor offers the protocol.
    fn ensure_cursor_dev(&mut self, qh: &QueueHandle<State>) {
        if self.cursor_dev.is_none() {
            if let (Some(mgr), Some(ptr)) = (&self.cursor_mgr, &self.pointer) {
                self.cursor_dev = Some(mgr.get_pointer(ptr, qh, ()));
            }
        }
    }

    /// Set the pointer image (no-op without cursor-shape-v1, or when the
    /// shape is already current).
    fn set_cursor(&mut self, shape: Shape) {
        if self.cursor_current == Some(shape) {
            return;
        }
        if let Some(dev) = &self.cursor_dev {
            dev.set_shape(self.ptr_serial, shape);
            self.cursor_current = Some(shape);
        }
    }

    /// The tooltip dwell elapsed: map the tooltip surface if the pointer is
    /// still on the same truncated button.
    fn show_tooltip(&mut self, qh: &QueueHandle<State>, bar_id: u32, tid: u32) {
        let (Some(comp), Some(ls)) = (&self.compositor, &self.layer_shell) else {
            return;
        };
        let Some(idx) = self.bar_index(bar_id) else {
            return;
        };
        let bar = &self.bars[idx];
        if bar.role != Role::Task || bar.hover != Hover::Task(tid) {
            return;
        }
        let Some(hit) = bar.task_hits.iter().find(|h| h.tid == tid) else {
            return;
        };
        let Some(t) = self
            .toplevels
            .iter()
            .find(|t| t.handle.id().protocol_id() == tid)
        else {
            return;
        };
        let src = if !t.title.trim().is_empty() { &t.title } else { &t.app_id };
        let text = sanitize_title(src);
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        // Cap to the output width (padding 16 + 8px of edge margins) so the
        // surface never maps wider than the screen and crops its own tail.
        let max_chars = ((bar.width as i32 - 24) / GLYPH_W).clamp(4, 64) as usize;
        let text: String = if text.chars().count() > max_chars {
            text.chars().take(max_chars - 1).chain(['.']).collect()
        } else {
            text.to_string()
        };

        let w = Canvas::text_width(&text) + 16;
        let h = GLYPH_H + 12;
        let center = (hit.x0 + hit.x1) / 2;
        let left = (center - w / 2).clamp(4, (bar.width as i32 - w - 4).max(4));

        let surface = comp.create_surface(qh, ());
        // Empty input region: the tooltip must never steal pointer focus from
        // the button it annotates (that would flicker enter/leave forever).
        let region = comp.create_region(qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        let layer = ls.get_layer_surface(
            &surface,
            Some(&bar.output),
            zwlr_layer_shell_v1::Layer::Overlay,
            "tooltip".into(),
            qh,
            TipId,
        );
        layer.set_anchor(Anchor::Bottom | Anchor::Left);
        layer.set_size(w as u32, h as u32);
        // Margins are relative to the work area, which already excludes the
        // bar's exclusive zone — bottom 6 floats just above the taskbar.
        layer.set_margin(0, 0, 6, left);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        surface.commit();
        self.tooltip = Some(Tooltip {
            surface,
            layer,
            width: 0,
            height: 0,
            map: std::ptr::null_mut(),
            map_len: 0,
            buffers: [None, None],
            busy: [false, false],
            next: 0,
            generation: 0,
            text,
            for_bar: bar_id,
            for_task: tid,
        });
    }

    /// (Re)allocate the tooltip's ARGB pool after a configure and paint it
    /// (its content is static for the tooltip's lifetime).
    fn configure_tip(&mut self, qh: &QueueHandle<State>, w: u32, h: u32) {
        let Some(shm) = self.shm.clone() else {
            return;
        };
        let w = w.max(1).min(fill_guard::MAX_BUFFER_DIM);
        let h = h.max(1).min(fill_guard::MAX_BUFFER_DIM);
        if matches!(self.tooltip.as_ref(), Some(tip) if tip.width == w && tip.height == h && !tip.map.is_null()) {
            self.render_tip();
            return;
        }
        let Some(total) = (w as usize)
            .checked_mul(4)
            .and_then(|s| s.checked_mul(h as usize))
            .and_then(|f| f.checked_mul(BUFFERS))
            .filter(|t| *t <= i32::MAX as usize)
        else {
            return;
        };
        let stride = w as usize * 4;
        let frame_size = stride * h as usize;
        let Some((map, fd)) = Self::map_shm_pool(total) else {
            return;
        };
        let generation = self.next_generation();
        let pool = shm.create_pool(fd.as_fd(), total as i32, qh, ());
        let mk = |i: usize| {
            pool.create_buffer(
                (i * frame_size) as i32,
                w as i32,
                h as i32,
                stride as i32,
                wl_shm::Format::Argb8888,
                qh,
                (TipId, i, generation),
            )
        };
        let buffers = [Some(mk(0)), Some(mk(1))];
        pool.destroy();

        let (old_map, old_len) = {
            let Some(tip) = self.tooltip.as_mut() else {
                unsafe { libc::munmap(map as *mut libc::c_void, total) };
                return;
            };
            for b in tip.buffers.iter_mut() {
                if let Some(b) = b.take() {
                    b.destroy();
                }
            }
            let old = (tip.map, tip.map_len);
            tip.width = w;
            tip.height = h;
            tip.map = map;
            tip.map_len = total;
            tip.buffers = buffers;
            tip.busy = [false, false];
            tip.next = 0;
            tip.generation = generation;
            old
        };
        self.retire_map(old_map, old_len);
        self.render_tip();
    }

    fn render_tip(&mut self) {
        let Some(tip) = self.tooltip.as_mut() else {
            return;
        };
        if tip.map.is_null() {
            return;
        }
        let (w, h) = (tip.width as usize, tip.height as usize);
        let frame_size = w * h * 4;
        let i = if !tip.busy[tip.next] {
            tip.next
        } else if !tip.busy[1 - tip.next] {
            1 - tip.next
        } else {
            return;
        };
        tip.next = 1 - i;
        tip.busy[i] = true;

        let Some(mut cv) = Canvas::try_new(w, h) else {
            tip.busy[i] = false;
            return;
        };
        draw_tooltip(&mut cv, w, h, &tip.text);
        let data: &mut [u8] =
            unsafe { std::slice::from_raw_parts_mut(tip.map.add(i * frame_size), frame_size) };
        if !cv.blit_argb(data) {
            tip.busy[i] = false;
            return;
        }
        if let Some(buf) = tip.buffers[i].as_ref() {
            tip.surface.attach(Some(buf), 0, 0);
            damage(&tip.surface, 1, 0, 0, w as i32, h as i32);
            tip.surface.commit();
        }
    }

    /// Route one wheel notch by pointer position.
    fn scroll_step(&mut self, dir: i32) {
        if self.ptr_on_popup {
            self.popup_scroll(dir);
            return;
        }
        if let Some(id) = self.ptr_bar {
            if let Some(idx) = self.bar_index(id) {
                if self.bars[idx].role == Role::Task {
                    self.cycle_windows(dir);
                }
            }
        }
    }
}

// ── Buffer helpers ───────────────────────────────────────────────────────────

/// Choose a released buffer. Returns None when the compositor still holds
/// both — the caller marks the bar dirty and repaints on the next Release
/// instead of writing into shm the compositor may be reading, which both
/// tears and corrupts the busy[] accounting via the stale Release that would
/// follow a double-attach.
fn pick_buffer(bar: &mut Bar) -> Option<usize> {
    let i = if !bar.busy[bar.next] {
        bar.next
    } else if !bar.busy[1 - bar.next] {
        1 - bar.next
    } else {
        return None;
    };
    bar.next = 1 - i;
    bar.busy[i] = true;
    Some(i)
}

fn commit_bar(bar: &mut Bar, i: usize) {
    if let Some(buf) = bar.buffers[i].as_ref() {
        let scale = bar.scale.max(1);
        let bw = (bar.width * scale) as i32;
        let bh = (bar.height * scale) as i32;
        bar.surface.attach(Some(buf), 0, 0);
        damage(&bar.surface, scale, 0, 0, bw, bh);
        bar.surface.commit();
    }
}

// ── Bottom bar: the waybar layout, evolved ───────────────────────────────────

/// Paint the taskbar: `◑ | [window buttons] … cpu N%  mem N%  [HH:MM]`.
/// Every button carries an icon slot (theme icon by app_id, letter badge as
/// fallback); buttons shrink to fit the available span instead of dropping
/// windows, collapsing to icon-only under pressure; hover and minimized
/// states render distinctly. Returns the launcher hitbox, per-button hitboxes
/// and the clock-pill hitbox.
fn draw_task(
    cv: &mut Canvas,
    w: usize,
    h: usize,
    items: &[TaskItem],
    m: &Metrics,
    hover: Hover,
    icons: &mut IconCache,
) -> ((i32, i32), Vec<TaskHit>, (i32, i32), (i32, i32), (i32, i32)) {
    cv.clear(BAR_BG);
    // border-top: 2px solid #6b5aa8
    cv.hline(0, 0, w as i32, BAR_RULE, 1.0);
    cv.hline(0, 1, w as i32, BAR_RULE, 1.0);

    let ty = (h as i32 + 2 - GLYPH_H) / 2; // text cell top, below the border
    let btn_h = (h as i32 - 10).max(1); // waybar: margin 3px + 2px border
    let btn_y = ((h as i32 - btn_h) / 2 + 1).max(0);

    // ── left: ◑ launcher (padding 0 10px, like #custom-launcher) ──
    let d = (h as i32 * 18) / 34; // ≈18px glyph in a 34px bar
    let ly = (h as i32 - d) / 2;
    let launcher_hit = (0, 10 + d + 10);
    if hover == Hover::Launcher {
        cv.round_rect_a(2, btn_y, launcher_hit.1 - 4, btn_h, 6, MENU_HOVER, 0.45);
    }
    cv.disc_half(10, ly, d, LAUNCH);

    // ── right side first, so the taskbar knows where to stop ──
    let left_min = launcher_hit.1 + 8;
    let mut rx = w as i32 - 4;
    let mut clock_hit = (0, 0);
    let mut vol_hit = (0, 0);
    let mut power_hit = (0, 0);

    {
        // Power button: far right
        let pw_size = 14;
        let pw_w = pw_size + 16;
        if rx - pw_w >= left_min {
            rx -= pw_w;
            let pill = if hover == Hover::Power { PILL_HOVER } else { PILL };
            cv.round_rect(rx, btn_y, pw_w, btn_h, 6, pill);
            cv.power_icon(rx + (pw_w - pw_size) / 2, btn_y + (btn_h - pw_size) / 2, pw_size, WHITE);
            power_hit = (rx, rx + pw_w);
            rx -= 10;
        }

        // clock: rounded pill, bold — click for calendar.
        let pw = Canvas::text_width(&m.clock) + 20;
        if rx - pw >= left_min {
            rx -= pw;
            let pill = if hover == Hover::Clock { PILL_HOVER } else { PILL };
            cv.round_rect(rx, btn_y, pw, btn_h, 6, pill);
            cv.text_bold(&m.clock, rx + 10, ty, TEXT);
            clock_hit = (rx, rx + pw);
            rx -= 10;
        }

        // Volume module
        if let Some(v) = m.vol {
            let vol_str = format!("vol {v}%");
            let vw = Canvas::text_width(&vol_str) + 10;
            if rx - vw >= left_min {
                rx -= vw;
                let col = if hover == Hover::Volume { WHITE } else { MUTED };
                cv.text(&vol_str, rx, ty, col);
                vol_hit = (rx, rx + vw);
                rx -= 10;
            }
        }

        let mem_s = format!("mem {}%", opt(m.mem));
        let mw = Canvas::text_width(&mem_s) + 10;
        if rx - mw >= left_min {
            rx -= mw;
            let col = if m.mem.unwrap_or(0) >= 90 { WARN } else { MUTED };
            cv.text(&mem_s, rx, ty, col);
            rx -= 10;
        }

        let cpu_s = format!("cpu {}%", opt(m.cpu));
        let cw = Canvas::text_width(&cpu_s) + 10;
        if rx - cw >= left_min {
            rx -= cw;
            let col = if m.cpu.unwrap_or(0) >= 90 { WARN } else { MUTED };
            cv.text(&cpu_s, rx, ty, col);
            rx -= 10;
        }
    }

    // ── taskbar window buttons ──
    let mut hits = Vec::new();
    let x0 = launcher_hit.1;
    let avail = (rx - 8) - x0;
    let n = items.len() as i32;
    let is = (btn_h - 6).clamp(12, 24);
    let icon_pad = is + 6;
    let mut widths: Vec<i32> = items
        .iter()
        .map(|it| Canvas::text_width(&it.label) + 16 + icon_pad)
        .collect();
    if n > 0 {
        let gaps = 4 * (n - 1);
        let natural: i32 = widths.iter().sum::<i32>() + gaps;
        if natural > avail {
            // Force equal widths so every window keeps a hitbox — never drop
            // buttons with `break` (that contradicted "shrink to fit").
            let each = if avail > gaps {
                ((avail - gaps) / n).max(1)
            } else {
                1
            };
            for bw in widths.iter_mut() {
                *bw = each;
            }
        }
    }
    let mut x = x0;
    for (k, it) in items.iter().enumerate() {
        let bw = widths[k];
        if bw <= 0 {
            continue;
        }
        // Clamp into the free span instead of silently skipping windows.
        let bw = bw.min((rx - 8 - x).max(1));
        if x >= rx - 8 {
            break;
        }
        let hovered = hover == Hover::Task(it.tid);
        if it.active {
            cv.round_rect(x, btn_y, bw, btn_h, 6, BTN_ACTIVE);
            cv.active_line(x, btn_y + btn_h - 2, bw, LAUNCH);
        } else if hovered {
            cv.round_rect_a(x, btn_y, bw, btn_h, 6, MENU_HOVER, 0.55);
        }
        let text_avail = bw - 16 - icon_pad;
        let icon_only = text_avail < GLYPH_W;
        let ix = if icon_only { x + (bw - is) / 2 } else { x + 8 };
        let iy = btn_y + (btn_h - is) / 2;
        match icons.get(&it.app_id, is as u32) {
            Some(pm) => cv.pixmap(ix, iy, &pm),
            None => {
                let ch = it.label.chars().next().unwrap_or('?');
                cv.badge(ix, iy, is, ch, PILL, LAUNCH);
            }
        }
        let mut cut = false;
        if !icon_only {
            let max_chars = (text_avail / GLYPH_W).max(1) as usize;
            let label: String = if it.label.chars().count() > max_chars {
                cut = true;
                it.label
                    .chars()
                    .take(max_chars.saturating_sub(1))
                    .chain(['.'])
                    .collect()
            } else {
                it.label.clone()
            };
            let fg = if it.active {
                WHITE
            } else if it.minimized {
                DIM
            } else {
                MUTED
            };
            cv.text(&label, x + 8 + icon_pad, ty, fg);
        }
        hits.push(TaskHit {
            x0: x,
            x1: x + bw,
            tid: it.tid,
            truncated: cut || icon_only || it.long,
        });
        x += bw + 4;
    }

    (launcher_hit, hits, clock_hit, vol_hit, power_hit)
}

// ── Top bar: system info ─────────────────────────────────────────────────────

fn metric(
    cv: &mut Canvas,
    right: i32,
    min_x: i32,
    ty: i32,
    h: i32,
    label: &str,
    gauge: Option<f32>,
    col: Rgb,
) -> Option<i32> {
    let tw = Canvas::text_width(label);
    let (gw, gpad) = if gauge.is_some() { (26, 6) } else { (0, 0) };
    let total = gw + gpad + tw;
    let x = right - total;
    if x < min_x {
        return None;
    }
    if let Some(f) = gauge {
        let ghh = 7;
        let gy = (h - ghh) / 2;
        cv.gauge(x, gy, gw, ghh, f, PILL);
    }
    cv.text(label, x + gw + gpad, ty, col);
    Some(x)
}

fn net_module(cv: &mut Canvas, right: i32, min_x: i32, ty: i32, h: i32, n: &NetRate) -> Option<i32> {
    let down = sysinfo::fmt_rate(n.down);
    let up = sysinfo::fmt_rate(n.up);
    let ts = 9;
    let ty_tri = (h - ts) / 2;
    let dw = Canvas::text_width(&down);
    let uw = Canvas::text_width(&up);
    let total = ts + 4 + dw + 10 + ts + 4 + uw;
    let x = right - total;
    if x < min_x {
        return None;
    }
    let mut cx = x;
    cv.triangle(cx, ty_tri, ts, false, LAUNCH);
    cx += ts + 4;
    cx += cv.text(&down, cx, ty, MUTED);
    cx += 10;
    cv.triangle(cx, ty_tri, ts, true, LAUNCH);
    cx += ts + 4;
    cv.text(&up, cx, ty, MUTED);
    Some(x)
}

fn draw_info(cv: &mut Canvas, w: usize, h: usize, m: &Metrics, hover: Hover) -> ((i32, i32), (i32, i32), (i32, i32)) {
    cv.clear(BAR_BG);
    cv.hline(0, h as i32 - 1, w as i32, BAR_RULE, 1.0);
    cv.hline(0, h as i32 - 2, w as i32, BAR_RULE, 1.0);

    let ty = (h as i32 - 2 - GLYPH_H) / 2;
    let hi = h as i32;
    let btn_h = hi - 10;
    let btn_y = (hi - btn_h) / 2;

    let d = (hi * 18) / 34;
    let ly = (hi - d) / 2;
    let mut lx = 10 + d + 8;
    let wordmark_w = Canvas::text_width("eclipse");
    let launcher_hit = (0, lx + wordmark_w + 4);
    if hover == Hover::Launcher {
        cv.round_rect_a(2, btn_y, launcher_hit.1 - 4, btn_h, 6, MENU_HOVER, 0.45);
    }
    cv.crescent(10, ly, d, LAUNCH);
    lx += cv.text_bold("eclipse", lx, ty, TEXT);

    if let Some(up) = &m.uptime {
        lx += 12;
        cv.vrule(lx, hi, BAR_RULE);
        lx += 12;
        lx += cv.text(&format!("up {up}"), lx, ty, MUTED);
    }
    if let Some(load) = m.load {
        lx += 12;
        cv.vrule(lx, hi, BAR_RULE);
        lx += 12;
        lx += cv.text(&format!("load {load:.2}"), lx, ty, MUTED);
    }

    let min_x = lx + 12;
    let mut rx = w as i32 - 4;
    let mut clock_hit = (0, 0);
    let mut power_hit = (0, 0);

    {
        let pw_size = 14;
        let pw_w = pw_size + 16;
        if rx - pw_w >= min_x {
            rx -= pw_w;
            let pill = if hover == Hover::Power { PILL_HOVER } else { PILL };
            cv.round_rect(rx, btn_y, pw_w, btn_h, 6, pill);
            cv.power_icon(rx + (pw_w - pw_size) / 2, btn_y + (btn_h - pw_size) / 2, pw_size, WHITE);
            power_hit = (rx, rx + pw_w);
            rx -= 10;
        }

        let pw = Canvas::text_width(&m.date) + 20;
        if rx - pw >= min_x {
            rx -= pw;
            let pill = if hover == Hover::Clock { PILL_HOVER } else { PILL };
            cv.round_rect(rx, btn_y, pw, btn_h, 6, pill);
            cv.text_bold(&m.date, rx + 10, ty, TEXT);
            clock_hit = (rx, rx + pw);
            rx -= 10;
        }
    }
    if let Some((b, ch)) = m.batt {
        let label = if ch { format!("bat {b}% +") } else { format!("bat {b}%") };
        let col = if b < 15 && !ch { WARN } else { MUTED };
        if let Some(x) = metric(cv, rx - 10, min_x, ty, hi, &label, Some(b as f32 / 100.0), col) {
            rx = x - 12;
            cv.vrule(rx, hi, BAR_RULE);
        }
    }
    if let Some(t) = m.temp {
        let col = if t >= 85 { WARN } else { MUTED };
        if let Some(x) = metric(cv, rx - 10, min_x, ty, hi, &format!("{t}°c"), None, col) {
            rx = x - 12;
            cv.vrule(rx, hi, BAR_RULE);
        }
    }
    if let Some(dk) = m.disk {
        if let Some(x) = metric(
            cv,
            rx - 10,
            min_x,
            ty,
            hi,
            &format!("disk {dk}%"),
            Some(dk as f32 / 100.0),
            MUTED,
        ) {
            rx = x - 12;
            cv.vrule(rx, hi, BAR_RULE);
        }
    }
    if let Some(n) = &m.net {
        if n.link {
            net_module(cv, rx - 10, min_x, ty, hi, n);
        }
    }

    (launcher_hit, clock_hit, power_hit)
}

// ── Launcher menu drawing ────────────────────────────────────────────────────

const APPS_PW: i32 = 320; // menu panel width
const APPS_HEADER_H: i32 = 34;
const APPS_SEARCH_H: i32 = 32;
const APPS_ROW_H: i32 = 30;
const APPS_PAD: i32 = 8;

/// Characters the search field can show at once — and therefore the cap on
/// the filter itself, so the two can never drift apart (a longer cap than the
/// field would silently hide what the user typed).
const APPS_FILTER_MAX: usize = ((APPS_PW - 20 - 24) / GLYPH_W) as usize;

/// How many app rows fit between the bars on an output `oh` tall.
fn apps_rows_fit(oh: i32, bar_h: i32) -> usize {
    let span = (oh - bar_h - 6) - (bar_h + 8);
    ((span - APPS_HEADER_H - APPS_SEARCH_H - APPS_PAD) / APPS_ROW_H).max(1) as usize
}

/// Paint the application menu overlay: a dim scrim over the whole output and
/// a rounded panel anchored above the bottom bar's launcher, holding a header,
/// a live search field, the filtered app rows (icon + name, scrolled to
/// `scroll`, `sel` highlighted) and a scrollbar when the list overflows.
/// Returns the panel rect and the row/arrow hitboxes.
#[allow(clippy::too_many_arguments)]
fn draw_apps(
    cv: &mut Canvas,
    ow: usize,
    oh: usize,
    bar_h: i32,
    all: &[apps::AppEntry],
    visible: &[usize],
    filter: &str,
    sel: usize,
    scroll: usize,
    icons: &mut IconCache,
) -> PopupFrame {
    // Dim backdrop (the canvas starts fully transparent).
    cv.fill_rect_a(0, 0, ow as i32, oh as i32, (0, 0, 0), 0.35);

    let pw = APPS_PW;
    let px = 8;
    let rows_fit = apps_rows_fit(oh as i32, bar_h);
    let shown = visible.len().clamp(1, rows_fit) as i32; // >=1: empty-state row
    let ph = APPS_HEADER_H + APPS_SEARCH_H + shown * APPS_ROW_H + APPS_PAD;
    let bottom = oh as i32 - bar_h - 6;
    let py = (bottom - ph).max(bar_h + 8);

    cv.round_rect_a(px, py, pw, ph, 12, MENU_PANEL, 0.98);
    // Violet accent rule under the header.
    cv.hline(px + 10, py + APPS_HEADER_H - 1, pw - 20, BAR_RULE, 0.7);

    // Header: crescent + title + result count.
    let icon = 18;
    cv.crescent(px + 12, py + (APPS_HEADER_H - icon) / 2, icon, LAUNCH);
    cv.text_bold(
        "aplicaciones",
        px + 12 + icon + 10,
        py + (APPS_HEADER_H - GLYPH_H) / 2,
        TEXT,
    );
    let count = visible.len().to_string();
    cv.text(
        &count,
        px + pw - 12 - Canvas::text_width(&count),
        py + (APPS_HEADER_H - GLYPH_H) / 2,
        DIM,
    );

    // Search field: what has been typed filters the list live.
    let sx = px + 10;
    let sy = py + APPS_HEADER_H + 6;
    let sw = pw - 20;
    let sh = APPS_SEARCH_H - 10;
    cv.round_rect(sx, sy, sw, sh, 6, PILL);
    let f_y = sy + (sh - GLYPH_H) / 2;
    let caret_x = if filter.is_empty() {
        cv.text("buscar aplicaciones", sx + 10, f_y, DIM);
        sx + 10
    } else {
        // popup_key caps the filter at APPS_FILTER_MAX — exactly what fits
        // here — so the whole filter always renders.
        sx + 10 + cv.text(filter, sx + 10, f_y, TEXT)
    };
    // Solid, not blinking: the overlay is output-sized, so a blink would cost
    // a full-screen repaint + composite every second (see the tick loop).
    cv.fill_rect_a(caret_x + 1, sy + 4, 2, sh - 8, LAUNCH, 0.9);

    // Rows (the scroll window over `visible`).
    let list_top = py + APPS_HEADER_H + APPS_SEARCH_H;
    let mut hits = Vec::new();
    if visible.is_empty() {
        cv.text(
            "sin resultados",
            px + 14,
            list_top + (APPS_ROW_H - GLYPH_H) / 2,
            DIM,
        );
    } else {
        let is = 20; // row icon slot
        for (row, &ai) in visible.iter().enumerate().skip(scroll).take(rows_fit) {
            let y = list_top + (row - scroll) as i32 * APPS_ROW_H;
            let selected = row == sel;
            if selected {
                cv.round_rect_a(px + 5, y + 1, pw - 10, APPS_ROW_H - 2, 6, MENU_HOVER, 1.0);
            }
            let e = &all[ai];
            // Icon slot: the entry's Icon=, else its name against the theme
            // index, else the letter badge.
            let iy = y + (APPS_ROW_H - is) / 2;
            let pm = e
                .icon
                .as_deref()
                .and_then(|n| icons.get(n, is as u32))
                .or_else(|| icons.get(&e.name, is as u32));
            match pm {
                Some(pm) => cv.pixmap(px + 12, iy, &pm),
                None => {
                    let ch = e.name.chars().next().unwrap_or('?');
                    cv.badge(px + 12, iy, is, ch, PILL, LAUNCH);
                }
            }
            let col = if selected { TEXT } else { MUTED };
            // Truncate long names to the panel width (font is ISO-8859-1, so a
            // plain '.' marks truncation, not the '…' glyph it lacks).
            let tx = px + 12 + is + 10;
            let max_chars = ((px + pw - 14 - 10 - tx) / GLYPH_W) as usize;
            let label: String = if e.name.chars().count() > max_chars {
                e.name.chars().take(max_chars.saturating_sub(1)).chain(['.']).collect()
            } else {
                e.name.clone()
            };
            cv.text(&label, tx, y + (APPS_ROW_H - GLYPH_H) / 2, col);
            hits.push((px + 5, y + 1, px + pw - 5, y + APPS_ROW_H - 1, Action::Row(row, ai)));
        }
        // Scrollbar when the list overflows the window.
        if visible.len() > rows_fit {
            let track_h = rows_fit as i32 * APPS_ROW_H - 8;
            let tx = px + pw - 7;
            let ty0 = list_top + 4;
            cv.round_rect_a(tx, ty0, 3, track_h, 1, MUTED, 0.15);
            let th = ((rows_fit as f32 / visible.len() as f32) * track_h as f32).max(18.0) as i32;
            let denom = (visible.len() - rows_fit).max(1) as f32;
            let toff = ((track_h - th) as f32 * scroll as f32 / denom) as i32;
            cv.round_rect_a(tx, ty0 + toff, 3, th, 1, LAUNCH, 0.6);
        }
    }

    ((px, py, pw, ph), hits)
}

// ── Calendar drawing ─────────────────────────────────────────────────────────

/// Paint the calendar overlay: a dim scrim and a month panel anchored to the
/// right edge — under the top bar when opened from the date pill, above the
/// bottom bar when opened from the clock. Today is highlighted; ◂/▸ hitboxes
/// shift the month. Returns the panel rect and the arrow hitboxes.
fn draw_calendar(
    cv: &mut Canvas,
    ow: usize,
    oh: usize,
    bar_h: i32,
    year: i32,
    month: u32,
    at_top: bool,
) -> PopupFrame {
    cv.fill_rect_a(0, 0, ow as i32, oh as i32, (0, 0, 0), 0.35);

    let (cell_w, cell_h) = (30, 24);
    let pad = 12;
    let pw = 7 * cell_w + 2 * pad;
    let header_h = 36;
    let wkd_h = 20;
    let ph = header_h + wkd_h + 6 * cell_h + pad;
    let px = (ow as i32 - pw - 8).max(0);
    let py = if at_top {
        bar_h + 6
    } else {
        (oh as i32 - bar_h - 6 - ph).max(bar_h + 6)
    };

    cv.round_rect_a(px, py, pw, ph, 12, MENU_PANEL, 0.98);
    cv.hline(px + 10, py + header_h - 1, pw - 20, BAR_RULE, 0.7);

    // Header: ◂ month year ▸.
    let ts = 10;
    let ay = py + (header_h - ts) / 2;
    cv.triangle_h(px + 16, ay, ts, true, LAUNCH);
    cv.triangle_h(px + pw - 16 - ts, ay, ts, false, LAUNCH);
    let title = format!("{} {}", sysinfo::MONTH_FULL[month as usize % 12], year);
    let tw = Canvas::text_width(&title);
    cv.text_bold(&title, px + (pw - tw) / 2, py + (header_h - GLYPH_H) / 2, TEXT);
    let hits = vec![
        (px + 4, py, px + 44, py + header_h, Action::PrevMonth),
        (px + pw - 44, py, px + pw - 4, py + header_h, Action::NextMonth),
    ];

    // Weekday header, Monday-first (lu ma mi ju vi sá do).
    const WKD: [&str; 7] = ["lu", "ma", "mi", "ju", "vi", "sá", "do"];
    for (i, wd) in WKD.iter().enumerate() {
        let x = px + pad + i as i32 * cell_w + (cell_w - Canvas::text_width(wd)) / 2;
        cv.text(wd, x, py + header_h + (wkd_h - GLYPH_H) / 2, DIM);
    }

    // Day grid (6 rows always, so the panel height is stable across months).
    let today = sysinfo::today();
    let first = sysinfo::first_weekday_mon0(year, month) as i32;
    let ndays = sysinfo::days_in_month(year, month) as i32;
    let gy = py + header_h + wkd_h;
    for d in 1..=ndays {
        let idx = first + d - 1;
        let (row, col) = (idx / 7, idx % 7);
        let x = px + pad + col * cell_w;
        let y = gy + row * cell_h;
        let s = d.to_string();
        let tx = x + (cell_w - Canvas::text_width(&s)) / 2;
        let dy = y + (cell_h - GLYPH_H) / 2;
        if today == Some((year, month, d as u32)) {
            cv.round_rect(x + 2, y + 1, cell_w - 4, cell_h - 2, 6, BAR_RULE);
            cv.text_bold(&s, tx, dy, WHITE);
        } else {
            // Weekend columns slightly dimmed, like every desktop calendar.
            let c = if col >= 5 { DIM } else { MUTED };
            cv.text(&s, tx, dy, c);
        }
    }

    ((px, py, pw, ph), hits)
}

// ── Power menu drawing ───────────────────────────────────────────────────────

fn draw_power_menu(
    cv: &mut Canvas,
    ow: usize,
    oh: usize,
    bar_h: i32,
    _at_top: bool,
) -> PopupFrame {
    cv.fill_rect_a(0, 0, ow as i32, oh as i32, (0, 0, 0), 0.35);

    let (pw, ph) = (180, 160);
    let px = (ow as i32 - pw - 10).max(0);
    let py = if _at_top {
        bar_h + 6
    } else {
        (oh as i32 - bar_h - 6 - ph).max(bar_h + 6)
    };

    cv.round_rect_a(px, py, pw, ph, 10, MENU_PANEL, 0.98);
    cv.round_rect_a(px, py, pw, ph, 10, BAR_RULE, 0.4);

    let items = [
        ("bloquear", Action::PowerLock),
        ("cerrar sesión", Action::PowerLogout),
        ("reiniciar", Action::PowerReboot),
        ("apagar", Action::PowerShutdown),
    ];

    let row_h = 34;
    let mut hits = Vec::new();
    let y0 = py + 12;

    for (i, (label, act)) in items.iter().enumerate() {
        let ry = y0 + i as i32 * row_h;
        let ix = px + 16;
        let iy = ry + (row_h - 14) / 2;
        let tx = px + 42;
        let ty = ry + (row_h - GLYPH_H) / 2;

        match act {
            Action::PowerLock => cv.lock_icon(ix, iy, 14, WHITE),
            Action::PowerLogout => cv.exit_icon(ix, iy, 14, WHITE),
            Action::PowerReboot => cv.reboot_icon(ix, iy, 14, WHITE),
            Action::PowerShutdown => cv.power_icon(ix, iy, 14, WHITE),
            _ => {}
        }
        cv.text(label, tx, ty, TEXT);
        hits.push((px + 6, ry, px + pw - 6, ry + row_h, *act));
    }

    ((px, py, pw, ph), hits)
}

// ── Window context menu drawing ──────────────────────────────────────────────

fn draw_task_menu(
    cv: &mut Canvas,
    ow: usize,
    oh: usize,
    bar_h: i32,
    tid: u32,
    title: &str,
    _at_top: bool,
) -> PopupFrame {
    cv.fill_rect_a(0, 0, ow as i32, oh as i32, (0, 0, 0), 0.35);

    let (pw, ph) = (200, 130);
    let px = (ow as i32 - pw - 10).max(0);
    let py = (oh as i32 - bar_h - 6 - ph).max(bar_h + 6);

    cv.round_rect_a(px, py, pw, ph, 10, MENU_PANEL, 0.98);
    cv.round_rect_a(px, py, pw, ph, 10, BAR_RULE, 0.4);

    let max_chars = ((pw - 24) / GLYPH_W) as usize;
    let head: String = if title.chars().count() > max_chars {
        title.chars().take(max_chars.saturating_sub(1)).chain(['.']).collect()
    } else {
        title.to_string()
    };
    cv.text_bold(&head, px + 12, py + 10, DIM);
    cv.hline(px + 10, py + 30, pw - 20, BAR_RULE, 0.5);

    let items = [
        ("enfocar", Action::TaskFocus(tid)),
        ("minimizar", Action::TaskMinimize(tid)),
        ("cerrar ventana", Action::TaskClose(tid)),
    ];

    let row_h = 30;
    let mut hits = Vec::new();
    let y0 = py + 34;

    for (i, (label, act)) in items.iter().enumerate() {
        let ry = y0 + i as i32 * row_h;
        let tx = px + 16;
        let ty = ry + (row_h - GLYPH_H) / 2;
        cv.text(label, tx, ty, TEXT);
        hits.push((px + 6, ry, px + pw - 6, ry + row_h, *act));
    }

    ((px, py, pw, ph), hits)
}

// ── Volume control popup drawing ─────────────────────────────────────────────

fn draw_volume_menu(
    cv: &mut Canvas,
    ow: usize,
    oh: usize,
    bar_h: i32,
    vol: u32,
    _at_top: bool,
) -> PopupFrame {
    cv.fill_rect_a(0, 0, ow as i32, oh as i32, (0, 0, 0), 0.35);

    let (pw, ph) = (180, 80);
    let px = (ow as i32 - pw - 60).max(0);
    let py = (oh as i32 - bar_h - 6 - ph).max(bar_h + 6);

    cv.round_rect_a(px, py, pw, ph, 10, MENU_PANEL, 0.98);

    cv.volume_icon(px + 14, py + 14, 16, WHITE, vol == 0);
    let label = format!("volumen {}%", vol);
    cv.text_bold(&label, px + 40, py + 16, TEXT);

    let mut hits = Vec::new();
    let gw = pw - 30;
    let gy = py + 48;
    cv.gauge(px + 15, gy, gw, 10, vol as f32 / 100.0, PILL);
    // One continuous hitbox — percent is derived from the click x.
    hits.push((px + 15, gy - 10, px + 15 + gw, gy + 20, Action::VolumeGauge));

    ((px, py, pw, ph), hits)
}

// ── Tooltip drawing ──────────────────────────────────────────────────────────

/// Paint the taskbar tooltip: a hairline-bordered dark pill with the window's
/// full title.
fn draw_tooltip(cv: &mut Canvas, w: usize, h: usize, text: &str) {
    cv.round_rect_a(0, 0, w as i32, h as i32, 6, BAR_RULE, 0.85);
    cv.round_rect_a(1, 1, w as i32 - 2, h as i32 - 2, 5, MENU_PANEL, 1.0);
    cv.text(text, 8, (h as i32 - GLYPH_H) / 2, TEXT);
}

// ── Small helpers ────────────────────────────────────────────────────────────

/// "42" or "--" for an optional percentage.
fn opt(v: Option<u32>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "--".into())
}

/// Indices into `all` whose names match the search filter
/// (accent/case-insensitive substring).
fn filter_apps(all: &[apps::AppEntry], filter: &str) -> Vec<usize> {
    let f = apps::norm_key(filter);
    (0..all.len())
        .filter(|&i| f.is_empty() || apps::norm_key(&all[i].name).contains(&f))
        .collect()
}

/// Printable ASCII for a bare evdev keycode (KEY_1=2 … KEY_M=50), assuming
/// the standard QWERTY core (letters/digits are position-stable across
/// layouts; good enough for menu filtering without pulling in xkb).
fn key_char(code: u32) -> Option<char> {
    Some(match code {
        2..=10 => (b'1' + (code - 2) as u8) as char, // KEY_1..KEY_9
        11 => '0',                                   // KEY_0
        16..=25 => b"qwertyuiop"[(code - 16) as usize] as char,
        30..=38 => b"asdfghjkl"[(code - 30) as usize] as char,
        44..=50 => b"zxcvbnm"[(code - 44) as usize] as char,
        57 => ' ',
        12 => '-',
        52 => '.',
        _ => return None,
    })
}

/// A short, font-renderable button label for a window: prefer the title (what
/// waybar's `{title:.18}` showed), fall back to app_id, capped so buttons stay
/// a sane width. Also reports whether the cap dropped characters (the hover
/// tooltip then shows the full title).
fn button_label(t: &Toplevel) -> (String, bool) {
    let src = if !t.title.trim().is_empty() {
        &t.title
    } else {
        &t.app_id
    };
    let src = sanitize_title(src);
    let src = src.trim();
    let src = if src.is_empty() { "window" } else { src };
    let mut s: String = src.chars().take(18).collect();
    let long = src.chars().count() > 18;
    if long {
        s.push('.');
    }
    (s, long)
}

/// Map a raw toplevel title onto FONT_9X15's ISO-8859-1 repertoire. Titles
/// arrive as arbitrary UTF-8: em/en dashes (Firefox's "page — browser"),
/// curly quotes, emoji, CJK, even control chars. Untranslatable glyphs would
/// each render as a full-width '?' — and a stray '\n' would draw a second
/// text line bleeding out of the button — so translate what has an ASCII
/// cousin, drop what is invisible, and collapse the rest into a single '?'.
fn sanitize_title(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for ch in src.chars() {
        match ch {
            '\u{2010}'..='\u{2015}' => out.push('-'),        // hyphens/dashes
            '\u{2018}' | '\u{2019}' => out.push('\''),       // curly single quotes
            '\u{201c}' | '\u{201d}' => out.push('"'),        // curly double quotes
            '\u{2026}' => out.push_str("..."),               // ellipsis
            // Invisible: zero-width chars, variation selectors, combining marks.
            '\u{200b}'..='\u{200f}' | '\u{fe00}'..='\u{fe0f}' | '\u{0300}'..='\u{036f}' => {}
            c if c.is_control() => out.push(' '),            // incl. \n, \t
            c if (c as u32) < 0x100 => out.push(c),          // Latin-1: has a glyph
            _ => {
                // Everything else has no glyph; collapse runs (a CJK title
                // becomes one '?', not one per codepoint).
                if !out.ends_with('?') {
                    out.push('?');
                }
            }
        }
    }
    out
}

// ── Wayland dispatch ─────────────────────────────────────────────────────────

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
            } => {
                match interface.as_str() {
                    "wl_compositor" => {
                        state.compositor = Some(registry.bind(name, version.min(4), qh, ()))
                    }
                    "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                    "zwlr_layer_shell_v1" => {
                        state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()))
                    }
                    "zwlr_foreign_toplevel_manager_v1" => {
                        state.foreign_mgr = Some(registry.bind(name, version.min(3), qh, ()))
                    }
                    "wp_cursor_shape_manager_v1" => {
                        state.cursor_mgr = Some(registry.bind(name, 1, qh, ()))
                    }
                    "wl_output" => {
                        let o: wl_output::WlOutput =
                            registry.bind(name, version.min(4), qh, name);
                        let info = OutputInfo {
                            output: o,
                            global_name: name,
                            scale: 1,
                            mode_w: 0,
                            mode_h: 0,
                        };
                        if !fill_guard::try_push_bounded(
                            &mut state.outputs,
                            info,
                            fill_guard::MAX_OUTPUTS,
                        ) {
                            eprintln!(
                                "lunarbar: ignoring wl_output past MAX_OUTPUTS={}",
                                fill_guard::MAX_OUTPUTS
                            );
                        }
                    }
                    "wl_seat" => {
                        // First seat wins — binding every seat leaks the prior
                        // pointer/keyboard and confuses hover tracking.
                        if state.seat.is_none() {
                            state.seat_name = Some(name);
                            state.seat = Some(registry.bind(name, version.min(5), qh, ()));
                        }
                    }
                    _ => {}
                }
            }
            wl_registry::Event::GlobalRemove { name } => {
                if state.seat_name == Some(name) {
                    state.clear_pointer_hover();
                    if let Some(d) = state.cursor_dev.take() {
                        d.destroy();
                    }
                    if let Some(p) = state.pointer.take() {
                        if p.version() >= 3 {
                            p.release();
                        }
                    }
                    if let Some(k) = state.keyboard.take() {
                        if k.version() >= 3 {
                            k.release();
                        }
                    }
                    if let Some(s) = state.seat.take() {
                        if s.version() >= 5 {
                            s.release();
                        }
                    }
                    state.seat_name = None;
                }
                if let Some(pos) = state.outputs.iter().position(|o| o.global_name == name) {
                    let oi = state.outputs.remove(pos);
                    state.bars.retain(|b| b.output_global != name);
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
                layer.ack_configure(serial);
                state.configure(qh, layer.id().protocol_id(), width, height);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                let id = layer.id().protocol_id();
                if let Some(pos) = state.bar_index(id) {
                    state.bars.remove(pos);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, (u32, usize, u64)> for State {
    fn event(
        state: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        (layer_id, i, generation): &(u32, usize, u64),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wl_buffer::Event::Release = event {
            if let Some(idx) = state.bar_index(*layer_id) {
                if state.bars[idx].generation != *generation {
                    return;
                }
                state.bars[idx].busy[*i] = false;
                // A skipped frame waits here, not for the next tick.
                if state.bars[idx].dirty {
                    state.bars[idx].dirty = false;
                    state.render(*layer_id);
                }
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                state.ptr_x = surface_x;
                state.ptr_y = surface_y;
                state.ptr_serial = serial;
                state.cursor_current = None; // shape must be re-set per enter
                state.scroll_acc = 0.0;
                state.ensure_cursor_dev(qh);
                state.ptr_on_popup = state
                    .popup
                    .as_ref()
                    .map(|m| m.surface.id() == surface.id())
                    .unwrap_or(false);
                state.ptr_bar = state
                    .bars
                    .iter()
                    .find(|b| b.surface.id() == surface.id())
                    .map(|b| b.layer.id().protocol_id());
                if state.ptr_on_popup {
                    state.popup_motion(surface_x as i32, surface_y as i32);
                } else if let Some(id) = state.ptr_bar {
                    state.bar_motion(id, surface_x as i32);
                } else {
                    state.set_cursor(Shape::Default);
                }
            }
            wl_pointer::Event::Leave { .. } => {
                if let Some(id) = state.ptr_bar.take() {
                    if let Some(idx) = state.bar_index(id) {
                        if state.bars[idx].hover != Hover::None {
                            state.bars[idx].hover = Hover::None;
                            state.render(id);
                        }
                    }
                }
                state.ptr_on_popup = false;
                state.scroll_acc = 0.0;
                state.tip_pending = None;
                state.tooltip = None;
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.ptr_x = surface_x;
                state.ptr_y = surface_y;
                if state.ptr_on_popup {
                    state.popup_motion(surface_x as i32, surface_y as i32);
                } else if let Some(id) = state.ptr_bar {
                    state.bar_motion(id, surface_x as i32);
                }
            }
            wl_pointer::Event::Button {
                button, state: bs, ..
            } => {
                let pressed = matches!(bs, WEnum::Value(wl_pointer::ButtonState::Pressed));
                if !pressed {
                    return;
                }
                let (x, y) = (state.ptr_x as i32, state.ptr_y as i32);
                state.tooltip = None;
                state.tip_pending = None;
                if state.ptr_on_popup {
                    if button == BTN_LEFT {
                        state.popup_click(x, y);
                    }
                    return;
                }
                let Some(id) = state.ptr_bar else { return };
                let Some(idx) = state.bar_index(id) else { return };
                let role = state.bars[idx].role;
                let (lx0, lx1) = state.bars[idx].launcher_hit;
                let (cx0, cx1) = state.bars[idx].clock_hit;
                let (vx0, vx1) = state.bars[idx].vol_hit;
                let (px0, px1) = state.bars[idx].power_hit;
                if button == BTN_LEFT && x >= lx0 && x < lx1 {
                    state.toggle_apps(qh);
                } else if button == BTN_LEFT && x >= cx0 && x < cx1 {
                    state.toggle_calendar(qh, role == Role::Info);
                } else if button == BTN_LEFT && x >= vx0 && x < vx1 {
                    state.toggle_volume(qh, role == Role::Info);
                } else if button == BTN_LEFT && x >= px0 && x < px1 {
                    state.toggle_power(qh, role == Role::Info);
                } else if role == Role::Task {
                    let hit = state.bars[idx]
                        .task_hits
                        .iter()
                        .find(|h| x >= h.x0 && x < h.x1)
                        .map(|h| h.tid);
                    if let Some(tid) = hit {
                        if button == BTN_RIGHT {
                            state.toggle_task_menu(qh, tid);
                        } else {
                            state.task_click(tid, button);
                        }
                    }
                }
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                value,
                ..
            } => {
                // Accumulate until one wheel notch, then step; trackpads
                // deliver many small deltas, wheels one ±15 per click.
                state.scroll_acc += value;
                while state.scroll_acc >= WHEEL_NOTCH {
                    state.scroll_acc -= WHEEL_NOTCH;
                    state.scroll_step(1);
                }
                while state.scroll_acc <= -WHEEL_NOTCH {
                    state.scroll_acc += WHEEL_NOTCH;
                    state.scroll_step(-1);
                }
            }
            _ => {}
        }
    }
}

// Popup overlay: its own layer surface (PopupId udata) and ARGB buffers.
impl Dispatch<ZwlrLayerSurfaceV1, PopupId> for State {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &PopupId,
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                state.configure_popup(qh, width, height);
            }
            zwlr_layer_surface_v1::Event::Closed => state.close_popup(),
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, (PopupId, usize, u64)> for State {
    fn event(
        state: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        (_, i, generation): &(PopupId, usize, u64),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wl_buffer::Event::Release = event {
            let mut redo = false;
            if let Some(popup) = state.popup.as_mut() {
                if popup.generation != *generation {
                    return;
                }
                popup.busy[*i] = false;
                if popup.dirty {
                    popup.dirty = false;
                    redo = true;
                }
            }
            if redo {
                state.render_popup();
            }
        }
    }
}

// Tooltip: its own layer surface (TipId udata) and ARGB buffers.
impl Dispatch<ZwlrLayerSurfaceV1, TipId> for State {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &TipId,
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                state.configure_tip(qh, width, height);
            }
            zwlr_layer_surface_v1::Event::Closed => state.tooltip = None,
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, (TipId, usize, u64)> for State {
    fn event(
        state: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        (_, i, generation): &(TipId, usize, u64),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wl_buffer::Event::Release = event {
            if let Some(tip) = state.tooltip.as_mut() {
                if tip.generation != *generation {
                    return;
                }
                tip.busy[*i] = false;
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        // Keys only reach us while the popup holds the keyboard (Exclusive);
        // raw evdev keycodes, no xkb needed.
        if let wl_keyboard::Event::Key { key, state: ks, .. } = event {
            let pressed = matches!(ks, WEnum::Value(wl_keyboard::KeyState::Pressed));
            if pressed {
                state.popup_key(key);
            }
        }
    }
}

// ── Foreign-toplevel (taskbar source) ────────────────────────────────────────

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            if state.toplevels.len() >= fill_guard::MAX_TRACKED_TOPLEVELS {
                eprintln!(
                    "lunarbar: toplevel list at MAX_TRACKED_TOPLEVELS={}; ignoring new window",
                    fill_guard::MAX_TRACKED_TOPLEVELS
                );
                toplevel.destroy();
            } else {
                state.toplevels.push(Toplevel {
                    handle: toplevel,
                    title: String::new(),
                    app_id: String::new(),
                    activated: false,
                    minimized: false,
                });
            }
        }
    }

    // The `toplevel` event creates a new handle object; tell wayland-client how
    // to type its user-data.
    wayland_client::event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event as E;
        let id = handle.id();
        match event {
            E::Title { title } => {
                if let Some(t) = state.toplevels.iter_mut().find(|t| t.handle.id() == id) {
                    t.title = fill_guard::truncate_chars(&title, fill_guard::MAX_TITLE_CHARS);
                }
            }
            E::AppId { app_id } => {
                if let Some(t) = state.toplevels.iter_mut().find(|t| t.handle.id() == id) {
                    t.app_id = fill_guard::truncate_chars(&app_id, fill_guard::MAX_APP_ID_CHARS);
                }
            }
            E::State { state: bytes } => {
                let mut activated = false;
                let mut minimized = false;
                for c in bytes.chunks_exact(4) {
                    let v = u32::from_ne_bytes([c[0], c[1], c[2], c[3]]);
                    if v == TOPLEVEL_STATE_ACTIVATED {
                        activated = true;
                    }
                    if v == TOPLEVEL_STATE_MINIMIZED {
                        minimized = true;
                    }
                }
                if let Some(t) = state.toplevels.iter_mut().find(|t| t.handle.id() == id) {
                    t.activated = activated;
                    t.minimized = minimized;
                }
            }
            E::Done => state.render_task_bars(),
            E::Closed => {
                handle.destroy();
                let tid = id.protocol_id();
                state.toplevels.retain(|t| t.handle.id() != id);
                // The window is gone: drop any tooltip or context menu bound
                // to it. Leaving the menu up would strand it on a dead title,
                // and since the compositor recycles object ids, a later window
                // could inherit this id and inherit the menu's "close window"
                // with it.
                state.tooltip = None;
                state.tip_pending = None;
                if matches!(&state.popup, Some(p) if matches!(p.kind, PopupKind::TaskMenu { tid: t, .. } if t == tid))
                {
                    state.close_popup();
                }
                state.render_task_bars();
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(State: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(State: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(State: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(State: ignore wl_region::WlRegion);
wayland_client::delegate_noop!(State: ignore WpCursorShapeManagerV1);
wayland_client::delegate_noop!(State: ignore WpCursorShapeDeviceV1);
impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        global_name: &u32,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        match event {
            wl_output::Event::Scale { factor } => {
                if let Some(oi) = state.outputs.iter_mut().find(|o| o.global_name == *global_name) {
                    oi.scale = factor.clamp(1, 8);
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
                    if let Some(oi) =
                        state.outputs.iter_mut().find(|o| o.global_name == *global_name)
                    {
                        oi.mode_w = width.max(0) as u32;
                        oi.mode_h = height.max(0) as u32;
                    }
                }
            }
            wl_output::Event::Done => {
                let scale = state
                    .outputs
                    .iter()
                    .find(|o| o.global_name == *global_name)
                    .map(|o| o.scale.clamp(1, 8) as u32)
                    .unwrap_or(1);
                let global = *global_name;
                for bar in state.bars.iter_mut().filter(|b| b.output_global == global) {
                    if bar.scale != scale && bar.configured {
                        bar.scale = scale;
                        // Force a rebuild on next configure/render path by
                        // clearing configured so ensure/configure refreshes.
                        bar.configured = false;
                    } else {
                        bar.scale = scale;
                    }
                }
                // Re-request configures by committing; compositors re-send size.
                for bar in state.bars.iter().filter(|b| b.output_global == global) {
                    bar.surface.commit();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        // Create/drop the pointer as the seat's POINTER capability toggles.
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            let has_pointer = caps.contains(wl_seat::Capability::Pointer);
            if has_pointer && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
                state.ensure_cursor_dev(qh);
            } else if !has_pointer {
                if let Some(d) = state.cursor_dev.take() {
                    d.destroy();
                }
                state.cursor_current = None;
                if let Some(p) = state.pointer.take() {
                    // release() exists from wl_pointer v3; below that the
                    // object just stays inert.
                    if p.version() >= 3 {
                        p.release();
                    }
                }
                state.clear_pointer_hover();
            }
            // Keyboard: drives the popup (search typing, arrows, Esc).
            let has_kbd = caps.contains(wl_seat::Capability::Keyboard);
            if has_kbd && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            } else if !has_kbd {
                if let Some(k) = state.keyboard.take() {
                    if k.version() >= 3 {
                        k.release();
                    }
                }
            }
        }
    }
}
wayland_client::delegate_noop!(State: ignore ZwlrLayerShellV1);

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
/// `XDG_RUNTIME_DIR`/`WAYLAND_DISPLAY` into our own environment so the app menu
/// launches its programs into a working session.
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
/// self-diagnosing (no dmesg needed). Same handler as lunarbg's: only
/// async-signal-safe `write(2)` + manual hex, then re-raise with the default
/// disposition so the shell still sees the real signal exit code (139).
fn install_crash_handler() {
    extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, _ctx: *mut libc::c_void) {
        let addr = unsafe { (*info).si_addr() } as usize;
        let mut buf = *b"lunarbar: FATAL signal 00 fault-addr 0x0000000000000000\n";
        buf[23] = b'0' + ((sig / 10) % 10) as u8;
        buf[24] = b'0' + (sig % 10) as u8;
        for i in 0..16 {
            let nibble = ((addr >> ((15 - i) * 4)) & 0xf) as u8;
            buf[39 + i] = if nibble < 10 {
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
        sa.sa_flags = libc::SA_SIGINFO;
        libc::sigemptyset(&mut sa.sa_mask);
        // Pass a real oldact buffer rather than null: some kernels (including
        // Eclipse OS's microkernel) unconditionally dereference the third
        // argument, causing a fault-addr 0x18 kernel page fault on null.
        libc::sigaction(libc::SIGSEGV, &sa, &mut old);
        libc::sigaction(libc::SIGBUS, &sa, &mut old);
        libc::sigaction(libc::SIGILL, &sa, &mut old);
    }
}

fn main() {
    install_crash_handler();
    // Minimum 26: the 15px font plus the h-10 pill height — anything shorter
    // draws glyphs taller than the pills that frame them.
    let height: u32 = std::env::var("LUNARBAR_HEIGHT")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|h| (26..=64).contains(h))
        .unwrap_or(34); // waybar's configured height
    let terminal = std::env::var("LUNARBAR_TERMINAL")
        .unwrap_or_else(|_| "/usr/local/bin/eclipse-terminal".into());

    // Offscreen preview for offline verification: render BOTH bars as they'd
    // sit on screen — top info bar, wallpaper gap, bottom taskbar — to a raw
    // XRGB8888 file. Spec is `path:WxH`; H is the full preview height.
    if let Ok(spec) = std::env::var("LUNARBAR_DUMP") {
        let (path, w, full_h) = match spec.rsplit_once(':') {
            Some((p, dims)) if dims.contains('x') => {
                let (ws, hs) = dims.split_once('x').unwrap();
                (p.to_string(), ws.parse().unwrap_or(1280), hs.parse().unwrap_or(220))
            }
            _ => (spec, 1280usize, 220usize),
        };
        // Clamp degenerate specs ("0x220", "1280x1") instead of tripping the
        // blit size assert — Canvas clamps its pixmap to >=1px but the
        // destination slices below are sized from the raw values.
        let w = w.clamp(64, 16384);
        let full_h = full_h.clamp(2 * height as usize, 16384);
        let bh = (height as usize).min(full_h / 2);
        let mut buf = vec![0u8; w * full_h * 4];

        // Wallpaper-tone fill for the gap between the bars (XRGB: B,G,R,X).
        const WALL: Rgb = (0x04, 0x07, 0x0e);
        for px in buf.chunks_exact_mut(4) {
            px[0] = WALL.2;
            px[1] = WALL.1;
            px[2] = WALL.0;
            px[3] = 0xff;
        }

        let mut cpu = CpuMeter::default();
        let mut net = NetMeter::default();
        let _ = cpu.sample();
        let _ = net.sample();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let m = Metrics {
            cpu: cpu.sample(),
            mem: sysinfo::mem_percent(),
            clock: sysinfo::clock_hhmm(),
            date: sysinfo::date_dm(),
            net: net.sample(),
            uptime: sysinfo::uptime(),
            load: sysinfo::loadavg(),
            disk: sysinfo::disk_root_percent(),
            temp: sysinfo::temp_c(),
            batt: sysinfo::battery(),
            vol: sysinfo::volume(),
        };

        // Top info bar occupies rows [0, bh).
        {
            let mut cv = Canvas::new(w, bh);
            draw_info(&mut cv, w, bh, &m, Hover::None);
            let _ = cv.blit_xrgb(&mut buf[..w * bh * 4]);
        }
        // Bottom taskbar occupies rows [full_h-bh, full_h) with sample windows
        // (one accented to exercise the ISO-8859-1 font, one minimized, one
        // hovered, one with a long truncated title).
        {
            let off = (full_h - bh) * w * 4;
            let mut cv = Canvas::new(w, bh);
            let mut tid = 0u32;
            let mut mk = |label: &str, app_id: &str, active, minimized, long| {
                tid += 1;
                TaskItem {
                    label: label.to_string(),
                    tid,
                    app_id: app_id.to_string(),
                    active,
                    minimized,
                    long,
                }
            };
            let sample = [
                mk("foot", "foot", true, false, false),
                mk("configuración", "config", false, false, false),
                mk("lunar editor", "lunar-editor", false, true, false),
                mk("notas - proyecto.", "notes", false, false, true),
            ];
            let mut ic = IconCache::default();
            // Hover the 4th sample (tid 4 — mk numbers them from 1), so the
            // preview exercises the hover highlight on a truncated button.
            draw_task(&mut cv, w, bh, &sample, &m, Hover::Task(4), &mut ic);
            let _ = cv.blit_xrgb(&mut buf[off..off + w * bh * 4]);
        }

        // Optional: composite open launcher menu (LUNARBAR_DUMP_MENU=1),
        // calendar (LUNARBAR_DUMP_CAL=1) or power menu (LUNARBAR_DUMP_POWER=1).
        let want_menu = std::env::var("LUNARBAR_DUMP_MENU").is_ok();
        let want_cal = std::env::var("LUNARBAR_DUMP_CAL").is_ok();
        let want_power = std::env::var("LUNARBAR_DUMP_POWER").is_ok();
        if want_menu || want_cal || want_power {
            let mut cv = Canvas::new(w, full_h);
            if want_power {
                draw_power_menu(&mut cv, w, full_h, bh as i32, false);
            } else if want_menu {
                let mut all = vec![apps::AppEntry {
                    name: "Terminal".into(),
                    exec: terminal.clone(),
                    icon: Some("utilities-terminal".into()),
                }];
                all.extend(apps::scan_apps(&terminal));
                if all.len() == 1 {
                    // No .desktop files in this environment: show sample rows
                    // so the preview still demonstrates the menu.
                    for n in ["Ajustes", "Archivos", "Navegador web", "Editor de texto"] {
                        all.push(apps::AppEntry { name: n.into(), exec: String::new(), icon: None });
                    }
                }
                let visible: Vec<usize> = (0..all.len()).collect();
                let mut ic = IconCache::default();
                draw_apps(&mut cv, w, full_h, bh as i32, &all, &visible, "", 1, 0, &mut ic);
            } else {
                let (y, mo, _) = sysinfo::today().unwrap_or((2026, 0, 1));
                draw_calendar(&mut cv, w, full_h, bh as i32, y, mo, false);
            }
            // Alpha-composite the overlay over the opaque preview.
            let mut over = vec![0u8; w * full_h * 4];
            let _ = cv.blit_argb(&mut over);
            for (dst, src) in buf.chunks_exact_mut(4).zip(over.chunks_exact(4)) {
                let a = src[3] as u32;
                if a == 0 {
                    continue;
                }
                for c in 0..3 {
                    // src is premultiplied: out = src + dst*(1-a)
                    dst[c] = (src[c] as u32 + dst[c] as u32 * (255 - a) / 255).min(255) as u8;
                }
                dst[3] = 0xff;
            }
        }

        std::fs::write(&path, buf).expect("write dump");
        eprintln!("lunarbar: dumped {w}x{full_h} XRGB8888 (both bars) to {path}");
        return;
    }

    let conn = match connect_wayland() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lunarbar: cannot connect to the Wayland compositor: {e}");
            std::process::exit(1);
        }
    };
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State {
        height,
        terminal,
        ..State::default()
    };
    // Prime the delta-based meters now: the registry roundtrip below provides
    // a natural sampling window, so the FIRST committed frame already shows a
    // real cpu% instead of a "cpu --%" flash.
    let _ = state.cpu.sample();
    let _ = state.net.sample();
    if let Err(e) = queue.roundtrip(&mut state) {
        eprintln!("lunarbar: initial roundtrip failed: {e}");
        std::process::exit(1);
    }
    if state.layer_shell.is_none() {
        eprintln!("lunarbar: compositor lacks zwlr_layer_shell_v1");
        std::process::exit(1);
    }
    if state.compositor.is_none() || state.shm.is_none() {
        eprintln!("lunarbar: missing wl_compositor or wl_shm");
        std::process::exit(1);
    }
    if state.foreign_mgr.is_none() {
        eprintln!("lunarbar: no wlr-foreign-toplevel-management — taskbar will be empty");
    }
    // Build the icon indexes before the bars map, so the first taskbar paint
    // does not stall the event loop on a cold-cache theme walk.
    state.vol = sysinfo::volume();
    state.icons.prewarm();
    state.refresh_metrics();

    // 1 Hz repaint for the metrics; interaction (hover, clicks, wheel, typing)
    // repaints immediately from its own events, so the bar feels live without
    // burning cycles between ticks.
    let interval = std::time::Duration::from_secs(1);
    let mut next_tick = std::time::Instant::now() + interval;

    loop {
        state.ensure_surfaces(&qh);
        if let Err(e) = queue.flush() {
            let recoverable = matches!(
                &e,
                wayland_client::backend::WaylandError::Io(io)
                    if io.kind() == std::io::ErrorKind::WouldBlock
            );
            if !recoverable {
                eprintln!("lunarbar: connection lost: {e}");
                std::process::exit(1);
            }
        }
        if let Some(guard) = queue.prepare_read() {
            // Sleep until the tick — or the tooltip dwell, whichever is first.
            let mut deadline = next_tick;
            if let Some((_, _, t0)) = state.tip_pending {
                deadline = deadline.min(t0 + TIP_DELAY);
            }
            // Round UP: truncating sub-ms waits asks for poll(0) and busy-spins.
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let timeout_ms = (left.as_micros().div_ceil(1000)).min(1000) as i32;
            let mut pfd = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut pfd, 1, timeout_ms.max(0)) };
            if ready > 0 {
                let _ = guard.read();
            } else {
                drop(guard);
                if ready < 0 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() != std::io::ErrorKind::Interrupted {
                        eprintln!("lunarbar: poll failed: {err}");
                        std::process::exit(1);
                    }
                }
            }
        }
        if let Err(e) = queue.dispatch_pending(&mut state) {
            eprintln!("lunarbar: protocol error: {e}");
            std::process::exit(1);
        }
        // Tooltip dwell elapsed while the pointer stayed on the same button?
        if let Some((id, k, t0)) = state.tip_pending {
            if t0.elapsed() >= TIP_DELAY {
                state.tip_pending = None;
                state.show_tooltip(&qh, id, k);
            }
        }
        if std::time::Instant::now() >= next_tick {
            state.refresh_metrics();
            state.tick = state.tick.wrapping_add(1);
            state.render_all();
            // Popups are static between interactions and repaint from their
            // own events. Nothing is redrawn here: the overlay is an
            // output-sized surface, so a per-second repaint would mean a
            // full-screen render, an ~8 MB swizzle and a whole-screen damage
            // (forcing a full re-composite) just to toggle a caret — the
            // caret is drawn solid instead.
            next_tick += interval;
            let now = std::time::Instant::now();
            if next_tick < now {
                next_tick = now + interval;
            }
        }
    }
}
