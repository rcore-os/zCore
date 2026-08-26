//! Implement INode for Stdin & Stdout
#![allow(dead_code)]

use super::ioctl::*;
use crate::{sync::Event, sync::EventBus};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicI32, AtomicU64, AtomicU8, Ordering};
use core::task::{Context, Poll};
use core::time::Duration;
use kernel_hal::console::{self, ConsoleWinSize};
use kernel_hal::user::{Error as UserError, UserInPtr, UserOutPtr};
use lazy_static::lazy_static;
use lock::Mutex;
use rcore_fs::vfs::*;
use zcore_drivers::prelude::{InputEvent, InputEventType};
use zircon_object::object::KernelObject;
use zircon_object::task::Thread;

// c_iflag
const IGNBRK: u32 = 0x0001;
const BRKINT: u32 = 0x0002;
const IGNPAR: u32 = 0x0004;
const PARMRK: u32 = 0x0008;
const INPCK: u32 = 0x0010;
const ISTRIP: u32 = 0x0020;
const INLCR: u32 = 0x0040;
const IGNCR: u32 = 0x0080;
const ICRNL: u32 = 0x0100;
const IUCLC: u32 = 0x0200;
const IXON: u32 = 0x0400;
const IXANY: u32 = 0x0800;
const IXOFF: u32 = 0x1000;
const IMAXBEL: u32 = 0x2000;
const IUTF8: u32 = 0x4000;

// c_oflag
const OPOST: u32 = 0x0001;
const OLCUC: u32 = 0x0002;
const ONLCR: u32 = 0x0004;
const OCRNL: u32 = 0x0008;
const ONOCR: u32 = 0x0010;
const ONLRET: u32 = 0x0020;
const OFILL: u32 = 0x0040;
const OFDEL: u32 = 0x0080;

// c_lflag
const ISIG: u32 = 0x0001;
const ICANON: u32 = 0x0002;
const XCASE: u32 = 0x0004;
const ECHO: u32 = 0x0008;
const ECHOE: u32 = 0x0010;
const ECHOK: u32 = 0x0020;
const ECHONL: u32 = 0x0040;
const NOFLSH: u32 = 0x0080;
const TOSTOP: u32 = 0x0100;
const ECHOCTL: u32 = 0x0200;
const ECHOPRT: u32 = 0x0400;
const ECHOKE: u32 = 0x0800;
const FLUSHO: u32 = 0x1000;
const PENDIN: u32 = 0x4000;
const IEXTEN: u32 = 0x8000;

// c_cc indices
const VINTR: usize = 0;
const VQUIT: usize = 1;
const VERASE: usize = 2;
const VKILL: usize = 3;
const VEOF: usize = 4;
const VTIME: usize = 5;
const VMIN: usize = 6;
const VSWTC: usize = 7;
const VSTART: usize = 8;
const VSTOP: usize = 9;
const VSUSP: usize = 10;
const VEOL: usize = 11;
const VREPRINT: usize = 12;
const VDISCARD: usize = 13;
const VWERASE: usize = 14;
const VLNEXT: usize = 15;
const VEOL2: usize = 16;

// Per-VT TTY state: each virtual terminal has its own termios and foreground
// process group, so e.g. a shell putting its terminal in raw mode on tty2 does
// not disturb the cooked shell on tty1.
struct TtyState {
    termios: Mutex<Termios>,
    fg_pgrp: AtomicI32,
    /// VT switch signalling mode (`VT_GETMODE` / `VT_SETMODE`).
    vt_mode: Mutex<VtMode>,
    /// Keyboard translation mode (`KDGKBMODE` / `KDSKBMODE`). When an X server
    /// sets `K_RAW`/`K_OFF`, the line discipline stops cooking key presses into
    /// this TTY (the raw events still reach userspace via `/dev/input/event*`).
    kbd_mode: AtomicI32,
    /// Scroll/Num/Caps LED bits last set via `KDSETLED`.
    kbd_leds: AtomicU8,
    /// Software flow control (IXON): output to this VT is paused after a `VSTOP`
    /// (Ctrl-S) and resumed by `VSTART` (Ctrl-Q).
    flow_stopped: AtomicBool,
}

lazy_static! {
    static ref TTY_STATES: Vec<TtyState> = (0..kernel_hal::console::NUM_VTS)
        .map(|_| TtyState {
            termios: Mutex::new(Termios::default_tty()),
            fg_pgrp: AtomicI32::new(0),
            vt_mode: Mutex::new(VtMode::auto()),
            kbd_mode: AtomicI32::new(K_XLATE),
            kbd_leds: AtomicU8::new(0),
            flow_stopped: AtomicBool::new(false),
        })
        .collect();
}

#[inline]
fn vt_clamp(vt: usize) -> usize {
    vt.min(kernel_hal::console::NUM_VTS - 1)
}

fn tty_termios(vt: usize) -> &'static Mutex<Termios> {
    &TTY_STATES[vt_clamp(vt)].termios
}

fn tty_fg_pgrp(vt: usize) -> &'static AtomicI32 {
    &TTY_STATES[vt_clamp(vt)].fg_pgrp
}

fn tty_vt_mode(vt: usize) -> &'static Mutex<VtMode> {
    &TTY_STATES[vt_clamp(vt)].vt_mode
}

fn tty_kbd_mode(vt: usize) -> i32 {
    TTY_STATES[vt_clamp(vt)].kbd_mode.load(Ordering::Relaxed)
}

/// Whether output to `vt` is currently paused by software flow control (IXON).
fn tty_flow_stopped(vt: usize) -> bool {
    TTY_STATES[vt_clamp(vt)]
        .flow_stopped
        .load(Ordering::Relaxed)
}

/// Whether VT `vt` is translating key presses into TTY characters. False while
/// an X server holds the keyboard in `K_RAW`/`K_OFF`/`K_MEDIUMRAW`.
fn tty_kbd_cooked(vt: usize) -> bool {
    matches!(tty_kbd_mode(vt), K_XLATE | K_UNICODE)
}

/// The VT an X server is driving in `K_MEDIUMRAW`, if any. kdrive (TinyX) puts
/// the keyboard of the VT it opened into medium-raw and reads keycodes from that
/// VT's tty. Routing is strictly by the *active* VT: medium-raw keycodes are
/// only delivered to the foreground VT when that VT is the one in medium-raw.
///
/// Earlier this fell back to "the first VT in medium-raw" whenever the active VT
/// wasn't, to paper over a feared `VT_OPENQRY`/`active_vt()` desync. But X takes
/// over the VT it was launched on (`VT_OPENQRY` returns the active VT and X sets
/// that same VT to medium-raw, so they are always in sync), and the fallback had
/// a nasty side effect: after the user switched to a text VT with Ctrl+Alt+Fx,
/// the X VT was still in medium-raw, so *every* keystroke kept being swallowed
/// into X's tty and the text console could never be typed into. Gating on the
/// active VT fixes that — keys go to X only while X is foreground, and to the
/// cooked console otherwise. Returns `None` for a normal cooked console.
fn medium_raw_vt() -> Option<usize> {
    let active = vt_clamp(kernel_hal::console::active_vt());
    if tty_kbd_mode(active) == K_MEDIUMRAW {
        Some(active)
    } else {
        None
    }
}

fn user_copy<T>(r: core::result::Result<T, UserError>) -> Result<T> {
    r.map_err(|_| FsError::InvalidParam)
}

/// `c_cc[idx] == 0` means the special character is disabled (`_POSIX_VDISABLE`).
#[inline]
fn cc_match(cc: &[u8; 19], idx: usize, c: u8) -> bool {
    let v = cc[idx];
    v != 0 && c == v
}

/// Shared TTY/console ioctl handling for every VT-backed device node
/// (`/dev/tty`, `/dev/tty[0-9]`, `/dev/console`, stdin and stdout). `vt` is the
/// virtual terminal the file refers to.
fn tty_ioctl(vt: usize, cmd: u32, data: usize) -> Result<usize> {
    match cmd as usize {
        TIOCGWINSZ => {
            user_copy(UserOutPtr::<ConsoleWinSize>::from(data).write(console::console_win_size()))?;
            Ok(0)
        }
        // Honor a caller-supplied window size. The framebuffer-derived default
        // is right for the on-screen graphic console but far too large for a
        // serial viewer (e.g. 227x113), so `resize`/`stty` over serial — and
        // the /etc/profile size probe — set the real terminal size here and
        // full-screen apps (nano, less, top) then render correctly. A 0x0 size
        // clears the override and restores the framebuffer size.
        TIOCSWINSZ => {
            let ws = user_copy(UserInPtr::<ConsoleWinSize>::from(data).read())?;
            let old = console::console_win_size();
            console::set_console_win_size(ws);
            let new = console::console_win_size();
            if old.ws_row != new.ws_row || old.ws_col != new.ws_col {
                let pgid = tty_fg_pgrp(vt).load(Ordering::Relaxed);
                if pgid > 0 {
                    let _ = crate::process::send_signal_to_pgrp(
                        pgid as usize,
                        crate::signal::Signal::SIGWINCH,
                    );
                }
            }
            Ok(0)
        }
        TCGETS => {
            user_copy(UserOutPtr::<Termios>::from(data).write(*tty_termios(vt).lock()))?;
            Ok(0)
        }
        TIOCSPGRP => {
            let pgid = unsafe { *(data as *const i32) };
            tty_fg_pgrp(vt).store(pgid, Ordering::Relaxed);
            Ok(0)
        }
        TIOCGPGRP => {
            let mut pgid = tty_fg_pgrp(vt).load(Ordering::Relaxed);
            if pgid == 0 {
                if let Some(arc) = kernel_hal::thread::get_current_thread() {
                    if let Ok(thread) = arc.downcast::<Thread>() {
                        pgid = crate::process::get_process_pgid(thread.proc().id()).unwrap_or(0)
                            as i32;
                    }
                }
            }
            if pgid == 0 {
                pgid = 1;
            }
            user_copy(UserOutPtr::<i32>::from(data).write(pgid))?;
            Ok(0)
        }
        TCSETS | TCSETSW => {
            *tty_termios(vt).lock() = user_copy(UserInPtr::<Termios>::from(data).read())?;
            Ok(0)
        }
        TCSETSF => {
            *tty_termios(vt).lock() = user_copy(UserInPtr::<Termios>::from(data).read())?;
            let sin = vt_stdin(vt);
            sin.buf.lock().clear();
            sin.canon_buf.lock().clear();
            sin.eof_pending.store(false, Ordering::Release);
            sin.vtime_deadline_ns.store(0, Ordering::Release);
            sin.eventbus.lock().clear(Event::READABLE);
            Ok(0)
        }
        // Argument is by value: TCIFLUSH=0, TCOFLUSH=1, TCIOFLUSH=2.
        TCFLSH => {
            const TCIFLUSH: usize = 0;
            const TCIOFLUSH: usize = 2;
            if data == TCIFLUSH || data == TCIOFLUSH {
                let sin = vt_stdin(vt);
                sin.buf.lock().clear();
                sin.canon_buf.lock().clear();
                sin.eof_pending.store(false, Ordering::Release);
                sin.vtime_deadline_ns.store(0, Ordering::Release);
                sin.eventbus.lock().clear(Event::READABLE);
            }
            Ok(0)
        }
        KDGETMODE => {
            unsafe { *(data as *mut i32) = console::kd_mode_vt(vt) as i32 };
            Ok(0)
        }
        KDSETMODE => {
            // `KDSETMODE` passes the mode (KD_TEXT/KD_GRAPHICS) by *value* in
            // the ioctl argument, not via a pointer — matches Linux
            // drivers/tty/vt/vt_ioctl.c. Dereferencing it would read a bogus
            // user address (e.g. KD_GRAPHICS == 1 → *0x1) and fault instead of
            // switching the console to graphics mode, which is exactly the step
            // an X server (TinyX/Xorg) performs to seize the display.
            let mode = data as u32;
            console::set_kd_mode_vt(vt, mode);
            Ok(0)
        }
        // X validates a console fd with KDGKBTYPE; reply with a PC keyboard.
        KDGKBTYPE => {
            unsafe { *(data as *mut u8) = KB_101 };
            Ok(0)
        }
        KDGKBMODE => {
            unsafe { *(data as *mut i32) = tty_kbd_mode(vt) };
            Ok(0)
        }
        KDSKBMODE => {
            // Like `KDSETMODE`, the keyboard mode (K_RAW/K_XLATE/K_OFF/…) is the
            // ioctl argument by value, not a pointer. X puts the keyboard into
            // K_RAW/K_OFF this way during console takeover.
            let mode = data as i32;
            TTY_STATES[vt_clamp(vt)]
                .kbd_mode
                .store(mode, Ordering::Relaxed);
            Ok(0)
        }
        // kdrive/TinyX reads the kernel keymap entry-by-entry to map medium-raw
        // keycodes to X keysyms. `struct kbentry { u8 kb_table; u8 kb_index;
        // u16 kb_value; }`: the caller fills kb_table/kb_index, we fill kb_value.
        KDGKBENT => {
            if data == 0 {
                return Err(FsError::InvalidParam);
            }
            let p = data as *mut u8;
            let (table, index) = unsafe { (*p, *p.add(1)) };
            let value = linux_keycode_to_evdev(index)
                .map(|evdev| kdgkbent_value(evdev, table))
                .unwrap_or(0);
            unsafe { *(p.add(2) as *mut u16) = value };
            Ok(0)
        }
        KDGETLED => {
            user_copy(
                UserOutPtr::<i32>::from(data)
                    .write(TTY_STATES[vt_clamp(vt)].kbd_leds.load(Ordering::Relaxed) as i32),
            )?;
            Ok(0)
        }
        KDSETLED => {
            TTY_STATES[vt_clamp(vt)]
                .kbd_leds
                .store(data as u8, Ordering::Relaxed);
            Ok(0)
        }
        KDMKTONE => Ok(0),
        // VT management. VT numbers in these ioctls are 1-based (tty1 == VT 1),
        // while the kernel tracks VTs 0-based internally.
        VT_OPENQRY => {
            // Hand the caller the active VT: an X server then takes over the
            // terminal it was launched from, which always has a device node.
            let vtno = kernel_hal::console::active_vt() as i32 + 1;
            unsafe { *(data as *mut i32) = vtno };
            Ok(0)
        }
        VT_GETMODE => {
            unsafe { *(data as *mut VtMode) = *tty_vt_mode(vt).lock() };
            Ok(0)
        }
        VT_SETMODE => {
            let mode = data as *const VtMode;
            if mode.is_null() {
                return Err(FsError::InvalidParam);
            }
            *tty_vt_mode(vt).lock() = unsafe { *mode };
            Ok(0)
        }
        VT_GETSTATE => {
            let active = kernel_hal::console::active_vt() as u16 + 1;
            // v_state is a bitmask of in-use VTs; bit N == VT N (bit 0 unused).
            let in_use = (((1u32 << kernel_hal::console::NUM_VTS) - 1) << 1) as u16;
            unsafe {
                *(data as *mut VtStat) = VtStat {
                    v_active: active,
                    v_signal: 0,
                    v_state: in_use,
                }
            };
            Ok(0)
        }
        VT_ACTIVATE => {
            if data >= 1 && data <= kernel_hal::console::num_vts() {
                kernel_hal::console::switch_vt(data - 1);
                crate::fs::devfs::drm::represent_if_owner_active();
            }
            Ok(0)
        }
        // Switches are synchronous, so the requested VT is already active.
        VT_WAITACTIVE => Ok(0),
        // No process-driven VT handshake to acknowledge, and our VTs are never
        // freed; accept the request so X's setup/teardown proceeds.
        VT_RELDISP | VT_DISALLOCATE => Ok(0),
        TIOCSCTTY | TIOCNOTTY => Ok(0),
        // Bytes available to read: the cooked input queue for this VT.
        FIONREAD => {
            let n = vt_stdin(vt).buf.lock().len() as i32;
            user_copy(UserOutPtr::<i32>::from(data).write(n))?;
            Ok(0)
        }
        // Console output is drawn synchronously, so nothing is ever queued.
        TIOCOUTQ => {
            user_copy(UserOutPtr::<i32>::from(data).write(0))?;
            Ok(0)
        }
        // Session ID of the terminal. We don't track sessions separately, so
        // report the foreground process group (same fallback as TIOCGPGRP).
        TIOCGSID => {
            let mut sid = tty_fg_pgrp(vt).load(Ordering::Relaxed);
            if sid <= 0 {
                sid = 1;
            }
            user_copy(UserOutPtr::<i32>::from(data).write(sid))?;
            Ok(0)
        }
        // Modem control lines. A VT has no real RS-232 lines, so report a
        // permanently-connected local terminal (DTR/RTS asserted, carrier up)
        // and accept writes as no-ops — matching how Linux treats a console.
        TIOCMGET => {
            let lines = TIOCM_DTR | TIOCM_RTS | TIOCM_CAR | TIOCM_CTS | TIOCM_DSR;
            user_copy(UserOutPtr::<i32>::from(data).write(lines))?;
            Ok(0)
        }
        TIOCMSET | TIOCMBIS | TIOCMBIC => Ok(0),
        // No real UART behind a VT, so all serial line counters are zero.
        TIOCGICOUNT => {
            user_copy(UserOutPtr::<SerialIcounter>::from(data).write(SerialIcounter::default()))?;
            Ok(0)
        }
        // Linux console multiplexor. The subcommand is the first byte of the
        // argument. We implement TIOCL_GETSHIFTSTATE (read modifier state),
        // which programs poll — sometimes in a tight loop — to read Shift/Ctrl/
        // Alt/AltGr without an evdev device; leaving it unhandled returned
        // ENOTTY and could spin a poller, flooding the console. Bits match
        // Linux's `shift_state` (KG_SHIFT/ALTGR/CTRL/ALT = 0/1/2/3).
        TIOCLINUX => {
            let p = data as *mut u8;
            if p.is_null() {
                return Err(FsError::InvalidParam);
            }
            let subcmd = unsafe { *p };
            match subcmd {
                TIOCL_GETSHIFTSTATE => {
                    let mut state = 0u8;
                    if SHIFT_DOWN.load(Ordering::SeqCst) {
                        state |= 1 << 0;
                    }
                    if ALTGR_DOWN.load(Ordering::SeqCst) {
                        state |= 1 << 1;
                    }
                    if CTRL_DOWN.load(Ordering::SeqCst) {
                        state |= 1 << 2;
                    }
                    if LEFT_ALT_DOWN.load(Ordering::SeqCst) {
                        state |= 1 << 3;
                    }
                    unsafe { *p = state };
                    Ok(0)
                }
                other => {
                    // Surface the actual subcommand once so an unhandled poller
                    // can be identified, then report EINVAL as Linux does.
                    static LOGGED: AtomicBool = AtomicBool::new(false);
                    if !LOGGED.swap(true, Ordering::Relaxed) {
                        warn!("TIOCLINUX: unhandled subcommand {}", other);
                    }
                    Err(FsError::InvalidParam)
                }
            }
        }
        _ => Err(FsError::NotSupported),
    }
}

/// Foreground process group of the *active* terminal (for signal delivery).
pub fn get_foreground_pgrp() -> i32 {
    tty_fg_pgrp(kernel_hal::console::active_vt()).load(Ordering::Relaxed)
}

pub fn set_foreground_pgrp(pgid: i32) {
    tty_fg_pgrp(kernel_hal::console::active_vt()).store(pgid, Ordering::Relaxed);
}

/// Seed a *specific* VT's foreground process group. Called when a per-VT shell
/// is spawned so it starts as the foreground job of its own tty. Without this
/// the VT's `fg_pgrp` stays 0 while the shell's pgrp is its pid, so `tcgetpgrp`
/// != `getpgrp`: an interactive `sh` then believes it is a *background* job and
/// spins sending itself `SIGTTIN` (a tight enter_uspace loop = wasted CPU/heat).
pub fn set_vt_foreground_pgrp(vt: usize, pgid: i32) {
    tty_fg_pgrp(vt).store(pgid, Ordering::Relaxed);
}

/// Replace termios on the active VT (used when `TCSETS` hits a non-tty fd).
pub fn set_active_vt_termios(termios: Termios) {
    *tty_termios(kernel_hal::console::active_vt()).lock() = termios;
}

/// Like [`set_active_vt_termios`] plus input-buffer flush for `TCSETSF`.
pub fn set_active_vt_termios_flush(termios: Termios) {
    let vt = kernel_hal::console::active_vt();
    *tty_termios(vt).lock() = termios;
    let sin = vt_stdin(vt);
    sin.buf.lock().clear();
    sin.canon_buf.lock().clear();
    sin.eventbus.lock().clear(Event::READABLE);
}

// Global Ctrl+C latch. Since many programs (e.g. udhcpc) never read stdin while running,
// we need a way for syscalls like recvfrom/poll to observe a pending terminal interrupt.
static CTRL_C_PENDING: AtomicBool = AtomicBool::new(false);
static CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static SHIFT_DOWN: AtomicBool = AtomicBool::new(false);
/// AltGr (Alt derecho) — layout `es` de Linux/XKB.
static ALTGR_DOWN: AtomicBool = AtomicBool::new(false);
/// Alt izquierdo — usado para la conmutación de VT (Ctrl+Alt+F1..F6).
static LEFT_ALT_DOWN: AtomicBool = AtomicBool::new(false);
static CAPSLOCK_ON: AtomicBool = AtomicBool::new(false);
/// Modo cursor de aplicación (DECCKM). Lo activa/desactiva la aplicación con
/// `ESC [ ? 1 h` / `ESC [ ? 1 l` (p. ej. el `smkx`/`rmkx` de ncurses). Cuando
/// está activo, las teclas de cursor emiten `ESC O x` en lugar de `ESC [ x`,
/// igual que `vc_decckm` en el VT de Linux.
static APP_CURSOR_KEYS: AtomicBool = AtomicBool::new(false);

#[allow(dead_code)]
pub fn ctrl_c_pending_take() -> bool {
    CTRL_C_PENDING.swap(false, Ordering::SeqCst)
}

#[allow(dead_code)]
pub fn ctrl_c_pending_set() {
    CTRL_C_PENDING.store(true, Ordering::SeqCst);
    wake_tty_intr_waiters();
}

/// Non-consuming check for multiplex wait loops.
pub fn ctrl_c_pending_peek() -> bool {
    CTRL_C_PENDING.load(Ordering::SeqCst)
}

lazy_static! {
    static ref TTY_INTR_WAKERS: Mutex<Vec<core::task::Waker>> = Mutex::new(Vec::new());
}

const MAX_TTY_INTR_WAKERS: usize = 64;

fn register_tty_waker_once(wakers: &mut Vec<core::task::Waker>, waker: &core::task::Waker) {
    if wakers.iter().any(|w| w.will_wake(waker)) {
        return;
    }
    if wakers.len() >= MAX_TTY_INTR_WAKERS {
        wakers.remove(0);
    }
    wakers.push(waker.clone());
}

pub fn wake_tty_intr_waiters() {
    let wakers: Vec<core::task::Waker> = core::mem::take(&mut *TTY_INTR_WAKERS.lock());
    for w in wakers {
        w.wake();
    }
}

pub fn register_tty_intr_waker(waker: core::task::Waker) {
    register_tty_waker_once(&mut TTY_INTR_WAKERS.lock(), &waker);
}

pub fn retain_tty_intr_waker(waker: &core::task::Waker) {
    TTY_INTR_WAKERS.lock().retain(|w| w.will_wake(waker));
}

/// Remove a wait's TTY-intr registration on Ready/`Drop` (see net clear_io_wait).
pub fn clear_tty_intr_waker(waker: &core::task::Waker) {
    TTY_INTR_WAKERS.lock().retain(|w| !w.will_wake(waker));
}

lazy_static! {
    /// One [`Stdin`] per virtual terminal. Building this also wires keyboard /
    /// UART input to the active terminal.
    pub static ref STDINS: Vec<Arc<Stdin>> = {
        let v: Vec<Arc<Stdin>> = (0..kernel_hal::console::NUM_VTS)
            .map(|i| Arc::new(Stdin::new(i)))
            .collect();

        // UART input goes to the active VT.
        if let Some(uart) = kernel_hal::drivers::all_uart().first() {
            uart.clone().subscribe(
                Box::new(move |_| {
                    while let Some(c) = uart.try_recv().unwrap_or(None) {
                        trace!("UART received byte: 0x{:02x}", c);
                        active_stdin().push(c as char);
                    }
                }),
                false,
            );
        }

        // Keyboards (USB / virtio / PS2): translated + routed by `handle_key_event`.
        for input in kernel_hal::drivers::all_input().as_vec().iter() {
            input.subscribe(Box::new(handle_key_event), false);
        }
        v
    };
    /// One [`Stdout`] per virtual terminal.
    pub static ref STDOUTS: Vec<Arc<Stdout>> = (0..kernel_hal::console::NUM_VTS)
        .map(|i| Arc::new(Stdout { vt: i }))
        .collect();
    /// Backwards-compatible alias for the first VT's stdin.
    pub static ref STDIN: Arc<Stdin> = STDINS[0].clone();
    /// Backwards-compatible alias for the first VT's stdout.
    pub static ref STDOUT: Arc<Stdout> = STDOUTS[0].clone();
}

/// Stdin of the currently active virtual terminal.
fn active_stdin() -> Arc<Stdin> {
    STDINS[vt_clamp(kernel_hal::console::active_vt())].clone()
}

/// Stdin of a specific virtual terminal.
pub fn vt_stdin(vt: usize) -> Arc<Stdin> {
    STDINS[vt_clamp(vt)].clone()
}

/// Stdout of a specific virtual terminal.
pub fn vt_stdout(vt: usize) -> Arc<Stdout> {
    STDOUTS[vt_clamp(vt)].clone()
}

/// Keyboard handler: tracks modifiers, switches VTs on Ctrl+Alt+F1..F6, handles
/// scrollback (Shift+PageUp/Down) and Ctrl+C, and feeds translated characters
/// to the active terminal.
fn handle_key_event(event: &InputEvent) {
    use zcore_drivers::input::input_event_codes::key::*;
    if event.event_type != InputEventType::Key {
        return;
    }
    // Linux input: value 1 = press, 0 = release, 2 = autorepeat. Track the
    // modifier state but don't return — the medium-raw path below has to emit
    // the modifier keycodes too.
    match event.code {
        KEY_LEFTCTRL | KEY_RIGHTCTRL => CTRL_DOWN.store(event.value != 0, Ordering::SeqCst),
        KEY_LEFTSHIFT | KEY_RIGHTSHIFT => SHIFT_DOWN.store(event.value != 0, Ordering::SeqCst),
        KEY_RIGHTALT => ALTGR_DOWN.store(event.value != 0, Ordering::SeqCst),
        KEY_LEFTALT => LEFT_ALT_DOWN.store(event.value != 0, Ordering::SeqCst),
        KEY_CAPSLOCK if event.value == 1 => {
            let on = CAPSLOCK_ON.load(Ordering::SeqCst);
            CAPSLOCK_ON.store(!on, Ordering::SeqCst);
        }
        _ => {}
    }

    // Ctrl+Alt+F1..F6 → switch virtual terminal. Checked before the medium-raw
    // hand-off below so the user can always leave an X session.
    if (event.value == 1 || event.value == 2)
        && CTRL_DOWN.load(Ordering::SeqCst)
        && LEFT_ALT_DOWN.load(Ordering::SeqCst)
    {
        let target = match event.code {
            KEY_F1 => Some(0),
            KEY_F2 => Some(1),
            KEY_F3 => Some(2),
            KEY_F4 => Some(3),
            KEY_F5 => Some(4),
            KEY_F6 => Some(5),
            _ => None,
        };
        if let Some(n) = target {
            if n < kernel_hal::console::num_vts() {
                kernel_hal::console::switch_vt(n);
                // If we switched back to the compositor's VT, re-blit its last
                // frame so it reappears at once (a no-op for a text target,
                // which the console repaints itself).
                crate::fs::devfs::drm::represent_if_owner_active();
            }
            return;
        }
    }

    // While an X server holds the keyboard in K_MEDIUMRAW (kdrive/TinyX), feed
    // it raw keycodes straight off the console: one byte per event, the keycode
    // with bit 7 set on release. Every key matters — presses, releases and
    // modifiers — but not autorepeat (value 2): kdrive generates its own
    // repeat. Keycodes ≥ 128 don't fit kdrive's single-byte reader; skip them.
    if let Some(vt) = medium_raw_vt() {
        if event.code < 0x80 && (event.value == 0 || event.value == 1) {
            let byte = (event.code as u8 & 0x7f) | if event.value == 0 { 0x80 } else { 0 };
            vt_stdin(vt).push_bytes(&[byte]);
        }
        return;
    }

    if event.value != 1 && event.value != 2 {
        return;
    }

    // Shift+PageUp / Shift+PageDown scrollback on the active VT.
    if SHIFT_DOWN.load(Ordering::SeqCst) {
        if event.code == KEY_PAGEUP {
            kernel_hal::console::scroll_graphic_console(1);
            return;
        }
        if event.code == KEY_PAGEDOWN {
            kernel_hal::console::scroll_graphic_console(-1);
            return;
        }
    }

    // Si el VT activo tiene el teclado en modo raw/off (p. ej. un servidor X lo
    // ha tomado con `KDSKBMODE`), no entregamos caracteres "cocidos" al TTY: los
    // eventos crudos siguen llegando a userspace por `/dev/input/event*`. Las
    // combinaciones de cambio de VT (Ctrl+Alt+F1..F6) ya se procesaron arriba.
    if !tty_kbd_cooked(kernel_hal::console::active_vt()) {
        return;
    }

    // Estado de modificadores, equivalente al `shift_state` de la capa keyboard
    // del kernel de Linux (drivers/tty/vt/keyboard.c).
    let mods = KeyMods {
        shift: SHIFT_DOWN.load(Ordering::SeqCst),
        altgr: ALTGR_DOWN.load(Ordering::SeqCst),
        caps: CAPSLOCK_ON.load(Ordering::SeqCst),
        ctrl: CTRL_DOWN.load(Ordering::SeqCst),
    };

    // Traducción keycode -> keysym, al estilo de los tipos KT_LATIN / KT_CUR /
    // KT_FN del VT de Linux. Ctrl+letra produce su carácter de control (Ctrl+C
    // = 0x03, que el TTY interpreta como VINTR/SIGINT).
    let stdin = active_stdin();
    match translate_key(event.code, mods) {
        Some(KeySym::Char(c)) => stdin.push(c),
        Some(KeySym::Cursor(final_byte)) => {
            // applkey(): ESC O x en modo cursor de aplicación (DECCKM activo),
            // ESC [ x en modo normal — igual que `applkey()` en el VT de Linux.
            let mid = if APP_CURSOR_KEYS.load(Ordering::SeqCst) {
                b'O'
            } else {
                b'['
            };
            stdin.push_bytes(&[0x1b, mid, final_byte]);
        }
        Some(KeySym::Func(seq)) => stdin.push_bytes(seq),
        None => {}
    }
}

/// Estado de modificadores para el layout español (XKB `es`).
#[derive(Clone, Copy)]
struct KeyMods {
    shift: bool,
    altgr: bool,
    caps: bool,
    ctrl: bool,
}

impl KeyMods {
    fn letter(self, lower: char) -> char {
        if self.caps ^ self.shift {
            lower.to_ascii_uppercase()
        } else {
            lower
        }
    }

    /// Elige entre cuatro niveles (como XKB: base, Shift, AltGr, Shift+AltGr).
    fn pick(self, base: char, shifted: char, altgr: char, shift_altgr: char) -> char {
        if self.altgr && self.shift {
            shift_altgr
        } else if self.altgr {
            altgr
        } else if self.shift {
            shifted
        } else {
            base
        }
    }
}

/// Resultado de traducir un keycode, análogo a los tipos de keysym del VT de
/// Linux (`drivers/tty/vt/keyboard.c`).
enum KeySym {
    /// Carácter imprimible (KT_LATIN/KT_LETTER). El bit de control ya está
    /// aplicado si correspondía.
    Char(char),
    /// Tecla de cursor (KT_CUR): se entrega la letra final (`A`..`D`, `H`, `F`).
    /// El emisor añade el prefijo `ESC O` (modo aplicación, DECCKM) o `ESC [`
    /// (modo normal), igual que `applkey()` en el kernel.
    Cursor(u8),
    /// Cadena fija de tecla de función / navegación (KT_FN): se emite tal cual.
    Func(&'static [u8]),
}

/// Traduce un keycode + modificadores a un keysym, replicando el modelo del VT
/// de Linux: teclas de cursor (KT_CUR), teclas de función con cadena fija
/// (KT_FN) y caracteres imprimibles (KT_LATIN) con el bit de control aplicado
/// como la columna `control` del keymap.
fn translate_key(code: u16, mods: KeyMods) -> Option<KeySym> {
    use zcore_drivers::input::input_event_codes::key::*;

    // Teclas de cursor: el prefijo lo decide el modo DECCKM al emitir.
    match code {
        KEY_UP => return Some(KeySym::Cursor(b'A')),
        KEY_DOWN => return Some(KeySym::Cursor(b'B')),
        KEY_RIGHT => return Some(KeySym::Cursor(b'C')),
        KEY_LEFT => return Some(KeySym::Cursor(b'D')),
        KEY_HOME => return Some(KeySym::Cursor(b'H')),
        KEY_END => return Some(KeySym::Cursor(b'F')),
        // Teclas con cadena fija (no dependen de DECCKM).
        KEY_PAGEUP => return Some(KeySym::Func(b"\x1b[5~")),
        KEY_PAGEDOWN => return Some(KeySym::Func(b"\x1b[6~")),
        KEY_INSERT => return Some(KeySym::Func(b"\x1b[2~")),
        KEY_DELETE => return Some(KeySym::Func(b"\x1b[3~")),
        KEY_ESC => return Some(KeySym::Func(b"\x1b")),
        _ => {}
    }

    // Carácter imprimible según el layout español.
    let c = input_event_to_char_es(code, mods)?;

    // Bit de control (KG_CTRL). El kernel toma el carácter de la columna
    // `control` del keymap; para el rango ASCII relevante equivale a estas
    // reglas: letras y `@ [ \ ] ^ _ ? espacio` -> carácter de control.
    if mods.ctrl {
        let ctrl_c = match c {
            ' ' | '@' => Some('\u{0}'), // NUL
            '?' => Some('\u{7f}'),      // DEL
            '[' => Some('\u{1b}'),      // ESC
            '\\' => Some('\u{1c}'),     // FS
            ']' => Some('\u{1d}'),      // GS
            '^' => Some('\u{1e}'),      // RS
            '_' => Some('\u{1f}'),      // US
            c if c.is_ascii_alphabetic() => Some(((c.to_ascii_uppercase() as u8) & 0x1f) as char),
            _ => None,
        };
        if let Some(ctrl_c) = ctrl_c {
            return Some(KeySym::Char(ctrl_c));
        }
    }

    Some(KeySym::Char(c))
}

/// Layout QWERTY español (España), alineado con `symbols/es` de xkeyboard-config.
fn input_event_to_char_es(code: u16, mods: KeyMods) -> Option<char> {
    use zcore_drivers::input::input_event_codes::key::*;
    match code {
        KEY_A => Some(mods.letter('a')),
        KEY_B => Some(mods.letter('b')),
        KEY_C => Some(mods.letter('c')),
        KEY_D => Some(mods.letter('d')),
        KEY_E => Some(mods.letter('e')),
        KEY_F => Some(mods.letter('f')),
        KEY_G => Some(mods.letter('g')),
        KEY_H => Some(mods.letter('h')),
        KEY_I => Some(mods.letter('i')),
        KEY_J => Some(mods.letter('j')),
        KEY_K => Some(mods.letter('k')),
        KEY_L => Some(mods.letter('l')),
        KEY_M => Some(mods.letter('m')),
        KEY_N => Some(mods.letter('n')),
        KEY_O => Some(mods.letter('o')),
        KEY_P => Some(mods.letter('p')),
        KEY_Q => Some(mods.letter('q')),
        KEY_R => Some(mods.letter('r')),
        KEY_S => Some(mods.letter('s')),
        KEY_T => Some(mods.letter('t')),
        KEY_U => Some(mods.letter('u')),
        KEY_V => Some(mods.letter('v')),
        KEY_W => Some(mods.letter('w')),
        KEY_X => Some(mods.letter('x')),
        KEY_Y => Some(mods.letter('y')),
        KEY_Z => Some(mods.letter('z')),
        KEY_1 => Some(mods.pick('1', '!', '|', '|')),
        KEY_2 => Some(mods.pick('2', '"', '@', '@')),
        KEY_3 => Some(mods.pick('3', '·', '#', '#')),
        KEY_4 => Some(mods.pick('4', '$', '~', '~')),
        KEY_5 => Some(mods.pick('5', '%', '€', '€')),
        KEY_6 => Some(mods.pick('6', '&', '¬', '¬')),
        KEY_7 => Some(mods.pick('7', '/', '{', '{')),
        KEY_8 => Some(mods.pick('8', '(', '[', '[')),
        KEY_9 => Some(mods.pick('9', ')', ']', ']')),
        KEY_0 => Some(mods.pick('0', '=', '}', '}')),
        KEY_MINUS => Some(mods.pick('\'', '?', '\\', '|')),
        KEY_EQUAL => Some(mods.pick('¡', '¿', '¡', '¿')),
        KEY_GRAVE => Some(mods.pick('º', 'ª', 'º', 'ª')),
        KEY_LEFTBRACE => Some(mods.pick('`', '^', '[', '{')),
        KEY_RIGHTBRACE => Some(mods.pick('+', '*', ']', '}')),
        KEY_BACKSLASH => Some(mods.pick('\\', '|', '|', '|')),
        KEY_SEMICOLON => Some(mods.pick('ñ', 'Ñ', '~', '`')),
        KEY_APOSTROPHE => Some(mods.pick('´', '¨', '{', '}')),
        KEY_102ND => Some(mods.pick('<', '>', '\\', '|')),
        KEY_COMMA => Some(mods.pick(',', ';', ',', ';')),
        KEY_DOT | KEY_KPDOT => Some(mods.pick('.', ':', '.', ':')),
        KEY_SLASH => Some(mods.pick('-', '_', '-', '_')),
        // Enter envía CR (0x0d), como una terminal real / la consola de Linux;
        // el flag de entrada ICRNL lo convierte a NL en modo canónico.
        KEY_ENTER | KEY_KPENTER => Some('\r'),
        KEY_SPACE => Some(' '),
        // Backspace envía DEL (0x7f), igual que la consola de Linux y el
        // `kbs=\177` de la terminfo xterm; además coincide con c_cc[VERASE].
        KEY_BACKSPACE => Some('\x7f'),
        KEY_TAB => Some('\t'),
        KEY_KP0 => Some('0'),
        KEY_KP1 => Some('1'),
        KEY_KP2 => Some('2'),
        KEY_KP3 => Some('3'),
        KEY_KP4 => Some('4'),
        KEY_KP5 => Some('5'),
        KEY_KP6 => Some('6'),
        KEY_KP7 => Some('7'),
        KEY_KP8 => Some('8'),
        KEY_KP9 => Some('9'),
        KEY_KPSLASH => Some('/'),
        KEY_KPASTERISK => Some('*'),
        KEY_KPMINUS => Some('-'),
        KEY_KPPLUS => Some('+'),
        _ => None,
    }
}

/// Map a Linux VT keycode (`kb_index` in `struct kbentry`) to evdev `KEY_*`.
fn linux_keycode_to_evdev(kc: u8) -> Option<u16> {
    use zcore_drivers::input::input_event_codes::key::*;
    match kc {
        1 => Some(KEY_ESC),
        2 => Some(KEY_1),
        3 => Some(KEY_2),
        4 => Some(KEY_3),
        5 => Some(KEY_4),
        6 => Some(KEY_5),
        7 => Some(KEY_6),
        8 => Some(KEY_7),
        9 => Some(KEY_8),
        10 => Some(KEY_9),
        11 => Some(KEY_0),
        12 => Some(KEY_MINUS),
        13 => Some(KEY_EQUAL),
        14 => Some(KEY_BACKSPACE),
        15 => Some(KEY_TAB),
        16 => Some(KEY_Q),
        17 => Some(KEY_W),
        18 => Some(KEY_E),
        19 => Some(KEY_R),
        20 => Some(KEY_T),
        21 => Some(KEY_Y),
        22 => Some(KEY_U),
        23 => Some(KEY_I),
        24 => Some(KEY_O),
        25 => Some(KEY_P),
        26 => Some(KEY_LEFTBRACE),
        27 => Some(KEY_RIGHTBRACE),
        28 => Some(KEY_ENTER),
        30 => Some(KEY_A),
        31 => Some(KEY_S),
        32 => Some(KEY_D),
        33 => Some(KEY_F),
        34 => Some(KEY_G),
        35 => Some(KEY_H),
        36 => Some(KEY_J),
        37 => Some(KEY_K),
        38 => Some(KEY_L),
        39 => Some(KEY_SEMICOLON),
        40 => Some(KEY_APOSTROPHE),
        41 => Some(KEY_GRAVE),
        43 => Some(KEY_BACKSLASH),
        44 => Some(KEY_Z),
        45 => Some(KEY_X),
        46 => Some(KEY_C),
        47 => Some(KEY_V),
        48 => Some(KEY_B),
        49 => Some(KEY_N),
        50 => Some(KEY_M),
        51 => Some(KEY_COMMA),
        52 => Some(KEY_DOT),
        53 => Some(KEY_SLASH),
        57 => Some(KEY_SPACE),
        59 => Some(KEY_F1),
        60 => Some(KEY_F2),
        61 => Some(KEY_F3),
        62 => Some(KEY_F4),
        63 => Some(KEY_F5),
        64 => Some(KEY_F6),
        65 => Some(KEY_F7),
        66 => Some(KEY_F8),
        67 => Some(KEY_F9),
        68 => Some(KEY_F10),
        87 => Some(KEY_F11),
        88 => Some(KEY_F12),
        _ => None,
    }
}

/// Build the `kb_value` for a `KDGKBENT` query (kdrive/TinyX keymap loader).
fn kdgkbent_value(keycode: u16, table: u8) -> u16 {
    use zcore_drivers::input::input_event_codes::key::*;
    const KT_LATIN: u16 = 0;
    const KT_SPEC: u16 = 2;
    const KT_CUR: u16 = 6;
    const KT_SHIFT: u16 = 7;
    const NO_SYMBOL: u16 = 0;
    const K_ENTER: u16 = (KT_SPEC << 8) | 1;
    const K_SHIFT: u16 = KT_SHIFT << 8;
    const K_ALTGR: u16 = (KT_SHIFT << 8) | 1;
    const K_CTRL: u16 = (KT_SHIFT << 8) | 2;
    const K_ALT: u16 = (KT_SHIFT << 8) | 3;
    const K_DOWN: u16 = KT_CUR << 8;
    const K_LEFT: u16 = (KT_CUR << 8) | 1;
    const K_RIGHT: u16 = (KT_CUR << 8) | 2;
    const K_UP: u16 = (KT_CUR << 8) | 3;

    match keycode {
        KEY_LEFTSHIFT | KEY_RIGHTSHIFT => return K_SHIFT,
        KEY_LEFTCTRL | KEY_RIGHTCTRL => return K_CTRL,
        KEY_LEFTALT => return K_ALT,
        KEY_RIGHTALT => return K_ALTGR,
        KEY_ENTER | KEY_KPENTER => return K_ENTER,
        KEY_ESC => return (KT_LATIN << 8) | 0x1b,
        KEY_UP => return K_UP,
        KEY_DOWN => return K_DOWN,
        KEY_LEFT => return K_LEFT,
        KEY_RIGHT => return K_RIGHT,
        _ => {}
    }

    let mods = KeyMods {
        shift: table & 1 != 0,
        altgr: table & 2 != 0,
        caps: false,
        ctrl: false,
    };
    match translate_key(keycode, mods) {
        Some(KeySym::Char(c)) if (c as u32) <= 0xff => c as u32 as u16,
        _ => NO_SYMBOL,
    }
}

/// Stdin struct, for Stdin buffer.
///
/// Design: `push()` is called from IRQ-handler callbacks (UART / xHCI HID).
/// To avoid deep nested spinlock chains from interrupt context (which caused
/// deadlocks after ~20-30 keystrokes), `push()` only touches the buffer lock
/// and sets an atomic flag — it does NOT touch the EventBus.  The EventBus
/// notification happens lazily from the executor side (SerialFuture / pop).
/// This is aligned with the Eclipse OS 1 pattern (usb_hid.rs → push_key),
/// where the ISR only writes to a circular buffer with interrupts disabled.
pub struct Stdin {
    /// Index of the virtual terminal this stdin belongs to.
    vt: usize,
    buf: Mutex<VecDeque<char>>,
    canon_buf: Mutex<VecDeque<char>>,
    eventbus: Mutex<EventBus>,
    /// Atomic flag set by `push()` so `SerialFuture` can detect new data
    /// without requiring `eventbus.lock()` from the IRQ path.
    data_ready: core::sync::atomic::AtomicBool,
    /// `VLNEXT` (literal-next, Ctrl-V) latch: when set, the next character is
    /// inserted verbatim, bypassing signal and line-editing processing.
    lnext: core::sync::atomic::AtomicBool,
    /// Canonical VEOF on an empty line: next `read` returns 0 (EOF).
    eof_pending: AtomicBool,
    /// Non-canonical `VMIN=0,VTIME>0`: monotonic deadline (ns) after which an
    /// empty read returns `Ok(0)`. Zero means no timer armed.
    vtime_deadline_ns: AtomicU64,
}

impl Stdin {
    /// The virtual terminal this stdin is bound to.
    pub fn vt(&self) -> usize {
        self.vt
    }

    fn new(vt: usize) -> Self {
        Self {
            vt,
            buf: Mutex::new(VecDeque::new()),
            canon_buf: Mutex::new(VecDeque::new()),
            eventbus: Mutex::new(EventBus::default()),
            data_ready: core::sync::atomic::AtomicBool::new(false),
            lnext: core::sync::atomic::AtomicBool::new(false),
            eof_pending: AtomicBool::new(false),
            vtime_deadline_ns: AtomicU64::new(0),
        }
    }

    /// True when a blocking reader can make progress (data, EOF, or VTIME done).
    fn read_ready(&self) -> bool {
        if self.can_read() || self.eof_pending.load(Ordering::Acquire) {
            return true;
        }
        let termios = tty_termios(self.vt).lock();
        let noncanon = termios.c_lflag & ICANON == 0;
        let vmin = termios.c_cc[VMIN];
        let vtime = termios.c_cc[VTIME];
        drop(termios);
        if noncanon && vmin == 0 {
            if vtime == 0 {
                return true;
            }
            let dl = self.vtime_deadline_ns.load(Ordering::Acquire);
            if dl != 0 && kernel_hal::timer::timer_now().as_nanos() as u64 >= dl {
                return true;
            }
        }
        false
    }

    /// Echo to this terminal's console.
    fn echo(&self, s: &str) {
        kernel_hal::console::vt_console_write_str(self.vt, s);
    }

    fn echo_char(&self, c: char) {
        let termios = tty_termios(self.vt).lock();
        let echo = termios.c_lflag & ECHO != 0;
        let echoctl = termios.c_lflag & ECHOCTL != 0;
        let opost = termios.c_oflag & OPOST != 0;
        let onlcr = termios.c_oflag & ONLCR != 0;
        drop(termios);

        if !echo {
            return;
        }

        match c {
            '\u{8}' | '\u{7f}' => {
                self.echo("\x08 \x08");
            }
            '\n' => {
                if opost && onlcr {
                    self.echo("\r\n");
                } else {
                    self.echo("\n");
                }
            }
            '\r' => {
                self.echo("\r");
            }
            c if c.is_control() => {
                if echoctl {
                    let mut s = [0u8; 2];
                    s[0] = b'^';
                    s[1] = (c as u8 + 64) & 0x7f;
                    if let Ok(s_str) = core::str::from_utf8(&s) {
                        self.echo(s_str);
                    }
                }
            }
            c => {
                let mut buf = [0u8; 4];
                self.echo(c.encode_utf8(&mut buf));
            }
        }
    }

    /// Mark new data available and wake any blocked reader / poller. Mirrors the
    /// notification dance used by the canonical/raw paths in [`push`].
    fn wake_readers(&self) {
        self.data_ready.store(true, Ordering::Release);
        if let Some(mut eb) = self.eventbus.try_lock() {
            self.data_ready.store(false, Ordering::Relaxed);
            eb.set(Event::READABLE);
        } else {
            wake_tty_intr_waiters();
        }
    }

    /// `VWERASE` (word erase, Ctrl-W): drop trailing whitespace, then the
    /// preceding word, from the pending canonical line, echoing the erase.
    fn word_erase(&self, lflag: u32) {
        let echo = lflag & ECHO != 0;
        let mut canon = self.canon_buf.lock();
        // Skip any trailing blanks first.
        while matches!(canon.back(), Some(' ') | Some('\t')) {
            canon.pop_back();
            if echo {
                self.echo("\x08 \x08");
            }
        }
        // Then erase the word itself, up to (not including) the next blank.
        while let Some(&ch) = canon.back() {
            if ch == ' ' || ch == '\t' {
                break;
            }
            canon.pop_back();
            if echo {
                self.echo("\x08 \x08");
            }
        }
    }

    /// `VREPRINT` (reprint, Ctrl-R): redraw the pending canonical line on a
    /// fresh line so the user can see input after noise (e.g. a kernel message).
    fn reprint(&self, lflag: u32) {
        if lflag & ECHO == 0 {
            return;
        }
        if lflag & ECHOCTL != 0 {
            self.echo("^R");
        }
        self.echo("\r\n");
        let canon = self.canon_buf.lock();
        let mut buf = [0u8; 4];
        for &ch in canon.iter() {
            self.echo(ch.encode_utf8(&mut buf));
        }
    }

    /// Insert a character verbatim (used after `VLNEXT`): no signal/edit
    /// interpretation. In canonical mode it joins the pending line; in raw mode
    /// it becomes immediately readable.
    fn input_literal(&self, c: char, lflag: u32) {
        if lflag & ICANON != 0 {
            self.canon_buf.lock().push_back(c);
            self.echo_char(c);
        } else {
            self.buf.lock().push_back(c);
            self.echo_char(c);
            self.wake_readers();
        }
    }

    /// Push a char into the Stdin buffer.
    ///
    /// Safe to call from IRQ context: acquires `buf` lock briefly (with
    /// interrupts disabled by the spinlock), sets an atomic flag, and
    /// *tries* to propagate to the EventBus via try_lock().  If the
    /// EventBus is contended the flag is left set for the next
    /// executor-side flush_ready_flag() call.
    pub fn push(&self, mut c: char) {
        let termios = tty_termios(self.vt).lock();
        let iflag = termios.c_iflag;
        let lflag = termios.c_lflag;
        let c_cc = termios.c_cc;
        drop(termios);

        // 1. Input translations
        if c == '\r' {
            if iflag & IGNCR != 0 {
                return;
            } else if iflag & ICRNL != 0 {
                c = '\n';
            }
        } else if c == '\n' && iflag & INLCR != 0 {
            c = '\r';
        }

        // 1b. Literal-next (VLNEXT): the previous keystroke was Ctrl-V, so take
        // this character verbatim, bypassing signal and line-editing handling.
        if self.lnext.swap(false, Ordering::Relaxed) {
            self.input_literal(c, lflag);
            return;
        }

        // 1c. Software flow control (IXON): VSTOP (Ctrl-S) pauses console output
        // and VSTART (Ctrl-Q) resumes it. These bytes are consumed, never
        // delivered. With IXANY, any input byte resumes paused output.
        if iflag & IXON != 0 {
            let state = &TTY_STATES[vt_clamp(self.vt)].flow_stopped;
            if cc_match(&c_cc, VSTOP, c as u8) {
                state.store(true, Ordering::Relaxed);
                return;
            }
            if cc_match(&c_cc, VSTART, c as u8) {
                state.store(false, Ordering::Relaxed);
                return;
            }
            if iflag & IXANY != 0 && state.swap(false, Ordering::Relaxed) {
                // Resumed by this byte; fall through to process it normally.
            }
        }

        // 1d. Discard (VDISCARD, Ctrl-O): toggles output flushing. The console
        // has no output queue to drop, so just consume the byte when IEXTEN is
        // on so it doesn't leak into the cooked line.
        if lflag & IEXTEN != 0 && c_cc[VDISCARD] != 0 && c as u8 == c_cc[VDISCARD] {
            return;
        }

        // 2. Signals
        if lflag & ISIG != 0 {
            // A job-control signal (Ctrl-C/Ctrl-\/Ctrl-Z) also lifts an IXON
            // output freeze, so the signalled process can run, print, or die
            // instead of staying blocked behind a Ctrl-S.
            if cc_match(&c_cc, VINTR, c as u8)
                || cc_match(&c_cc, VQUIT, c as u8)
                || cc_match(&c_cc, VSUSP, c as u8)
            {
                TTY_STATES[vt_clamp(self.vt)]
                    .flow_stopped
                    .store(false, Ordering::Relaxed);
            }
            if cc_match(&c_cc, VINTR, c as u8) {
                ctrl_c_pending_set();
                let pgid = tty_fg_pgrp(self.vt).load(Ordering::Relaxed);
                if pgid > 0 {
                    // Whole foreground group, not just the leader — otherwise a
                    // shell child like `ping` never sees the Ctrl-C.
                    let _ = crate::process::send_signal_to_pgrp(
                        pgid as usize,
                        crate::signal::Signal::SIGINT,
                    );
                }
                if lflag & NOFLSH == 0 {
                    self.buf.lock().clear();
                    self.canon_buf.lock().clear();
                    self.eof_pending.store(false, Ordering::Release);
                }
                if lflag & ECHO != 0 {
                    self.echo("^C\n");
                }
                // Wake waiters without latching READABLE on an empty queue
                // (that caused busy-loops in blocking `read`).
                if let Some(mut eb) = self.eventbus.try_lock() {
                    eb.clear(Event::READABLE);
                }
                wake_tty_intr_waiters();
                return;
            }
            if cc_match(&c_cc, VQUIT, c as u8) {
                let pgid = tty_fg_pgrp(self.vt).load(Ordering::Relaxed);
                if pgid > 0 {
                    let _ = crate::process::send_signal_to_pgrp(
                        pgid as usize,
                        crate::signal::Signal::SIGQUIT,
                    );
                }
                if lflag & NOFLSH == 0 {
                    self.buf.lock().clear();
                    self.canon_buf.lock().clear();
                    self.eof_pending.store(false, Ordering::Release);
                }
                if lflag & ECHO != 0 {
                    self.echo("^\\\n");
                }
                if let Some(mut eb) = self.eventbus.try_lock() {
                    eb.clear(Event::READABLE);
                }
                wake_tty_intr_waiters();
                return;
            }
            if cc_match(&c_cc, VSUSP, c as u8) {
                let pgid = tty_fg_pgrp(self.vt).load(Ordering::Relaxed);
                if pgid > 0 {
                    let _ = crate::process::send_signal_to_pgrp(
                        pgid as usize,
                        crate::signal::Signal::SIGTSTP,
                    );
                }
                if lflag & NOFLSH == 0 {
                    self.buf.lock().clear();
                    self.canon_buf.lock().clear();
                    self.eof_pending.store(false, Ordering::Release);
                }
                if lflag & ECHO != 0 {
                    self.echo("^Z\n");
                }
                if let Some(mut eb) = self.eventbus.try_lock() {
                    eb.clear(Event::READABLE);
                }
                wake_tty_intr_waiters();
                return;
            }
        }

        // 3. Canon vs Raw mode
        if lflag & ICANON != 0 {
            // Extended line editing (VWERASE / VREPRINT / VLNEXT) is gated on
            // IEXTEN, as in Linux n_tty. A c_cc of 0 means the char is disabled.
            let iexten = lflag & IEXTEN != 0;
            if iexten && c_cc[VWERASE] != 0 && c as u8 == c_cc[VWERASE] {
                self.word_erase(lflag);
            } else if iexten && c_cc[VREPRINT] != 0 && c as u8 == c_cc[VREPRINT] {
                self.reprint(lflag);
            } else if iexten && c_cc[VLNEXT] != 0 && c as u8 == c_cc[VLNEXT] {
                self.lnext.store(true, Ordering::Relaxed);
                if lflag & ECHO != 0 && lflag & ECHOCTL != 0 {
                    // Show "^" with the cursor parked on it until the quoted
                    // char arrives (Linux echoes ^ then a backspace).
                    self.echo("^\x08");
                }
            } else if cc_match(&c_cc, VERASE, c as u8) {
                let mut canon = self.canon_buf.lock();
                if let Some(_popped) = canon.pop_back() {
                    if lflag & ECHO != 0 {
                        if lflag & ECHOE != 0 {
                            self.echo("\x08 \x08");
                        } else {
                            let mut buf = [0u8; 4];
                            let erase_char = (c_cc[VERASE] as char).encode_utf8(&mut buf);
                            self.echo(erase_char);
                        }
                    }
                }
            } else if cc_match(&c_cc, VKILL, c as u8) {
                let mut canon = self.canon_buf.lock();
                let len = canon.len();
                canon.clear();
                if lflag & ECHO != 0 {
                    if lflag & ECHOKE != 0 {
                        for _ in 0..len {
                            self.echo("\x08 \x08");
                        }
                    } else if lflag & ECHOK != 0 {
                        self.echo("\n");
                    }
                }
            } else if cc_match(&c_cc, VEOF, c as u8) {
                let mut canon = self.canon_buf.lock();
                let empty = canon.is_empty();
                let mut buf = self.buf.lock();
                while let Some(ch) = canon.pop_front() {
                    buf.push_back(ch);
                }
                drop(buf);
                drop(canon);
                if empty {
                    // Empty line + VEOF → EOF for the next reader (Linux n_tty).
                    self.eof_pending.store(true, Ordering::Release);
                }
                self.wake_readers();
            } else {
                self.canon_buf.lock().push_back(c);
                self.echo_char(c);
                // A line is delivered to readers on newline or on either of the
                // configurable end-of-line delimiters (VEOL / VEOL2).
                let is_eol = c == '\n'
                    || (c_cc[VEOL] != 0 && c as u8 == c_cc[VEOL])
                    || (c_cc[VEOL2] != 0 && c as u8 == c_cc[VEOL2]);
                if is_eol {
                    let mut canon = self.canon_buf.lock();
                    let mut buf = self.buf.lock();
                    while let Some(ch) = canon.pop_front() {
                        buf.push_back(ch);
                    }
                    self.wake_readers();
                }
            }
        } else {
            // Raw mode
            self.buf.lock().push_back(c);
            self.echo_char(c);
            // Wake readers
            self.data_ready.store(true, Ordering::Release);
            if let Some(mut eb) = self.eventbus.try_lock() {
                self.data_ready.store(false, Ordering::Relaxed);
                eb.set(Event::READABLE);
            } else {
                wake_tty_intr_waiters();
            }
        }
    }

    /// Drain the atomic flag and propagate to EventBus.
    /// Called from executor context (SerialFuture::poll, pop, executor loop).
    pub fn flush_ready_flag(&self) {
        if self.data_ready.swap(false, Ordering::Acquire) {
            self.eventbus.lock().set(Event::READABLE);
        }
    }

    /// pop a char from the Stdin buffer
    pub fn pop(&self) -> char {
        self.flush_ready_flag();
        let mut buf_lock = self.buf.lock();
        let c = buf_lock.pop_front().unwrap();
        if buf_lock.is_empty() {
            self.eventbus.lock().clear(Event::READABLE);
        }
        c
    }

    /// specify whether the Stdin buffer is readable
    pub fn can_read(&self) -> bool {
        !self.buf.lock().is_empty()
    }

    /// Push raw bytes into stdin without echo (TTY query responses for userland).
    pub fn push_bytes(&self, bytes: &[u8]) {
        let mut buf = self.buf.lock();
        for &b in bytes {
            buf.push_back(b as char);
        }
        drop(buf);
        self.data_ready.store(true, Ordering::Release);
        if let Some(mut eb) = self.eventbus.try_lock() {
            self.data_ready.store(false, Ordering::Relaxed);
            eb.set(Event::READABLE);
        } else {
            // EventBus contended: leave `data_ready` set and nudge any waiter so
            // the bytes don't sit in the buffer until an unrelated event runs
            // the executor. This matters for kdrive/TinyX, whose only wakeup in
            // medium-raw mode is the keystroke we just pushed.
            wake_tty_intr_waiters();
        }
    }
}

/// Helper function to post-process output data (e.g. translating \n to \r\n if OPOST and ONLCR are set)
fn tty_write_out(vt: usize, buf: &[u8]) {
    // Honor software flow control: while output is stopped by VSTOP (Ctrl-S),
    // block here until a VSTART (Ctrl-Q) keystroke clears the flag. Keyboard
    // IRQs keep firing while we spin, so the flag can still be cleared. Bail out
    // on a pending Ctrl-C so a process blocked on a frozen terminal stays
    // killable (the SIGINT will be handled once this syscall returns).
    while tty_flow_stopped(vt) {
        if ctrl_c_pending_peek() {
            break;
        }
        core::hint::spin_loop();
    }
    let termios = tty_termios(vt).lock();
    let opost = termios.c_oflag & OPOST != 0;
    let onlcr = termios.c_oflag & ONLCR != 0;
    drop(termios);

    if opost && onlcr {
        let mut start = 0;
        for (i, &b) in buf.iter().enumerate() {
            if b == b'\n' {
                if i > start {
                    let s = unsafe { core::str::from_utf8_unchecked(&buf[start..i]) };
                    kernel_hal::console::vt_console_write_str(vt, s);
                }
                kernel_hal::console::vt_console_write_str(vt, "\r\n");
                start = i + 1;
            }
        }
        if start < buf.len() {
            let s = unsafe { core::str::from_utf8_unchecked(&buf[start..]) };
            kernel_hal::console::vt_console_write_str(vt, s);
        }
    } else {
        let s = unsafe { core::str::from_utf8_unchecked(buf) };
        kernel_hal::console::vt_console_write_str(vt, s);
    }
}

/// Track DEC private modes and answer DSR status (`CSI 5 n`).
///
/// Do **not** synthesize Cursor Position Reports for `CSI 6 n`: QEMU `-serial`
/// stdio/pty forwards those to the host terminal, which replies with the real
/// size. Injecting `\e[1;1R` races ahead of that reply, so `/etc/profile`'s
/// resize probe runs `stty rows 1 cols 1` and full-screen apps (nano) exit.
fn tty_handle_outgoing(vt: usize, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let mut need_status = false;
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'[' {
            // CSI ? Pm (h|l): modos privados DEC. Rastreamos DECCKM (modo 1)
            // para alternar el prefijo de las teclas de cursor, como el VT
            // de Linux con `\E[?1h` (smkx) / `\E[?1l` (rmkx).
            if i + 2 < data.len() && data[i + 2] == b'?' {
                let mut j = i + 3;
                let mut num: u32 = 0;
                let mut has_digit = false;
                while j < data.len() && data[j].is_ascii_digit() {
                    num = num.saturating_mul(10) + (data[j] - b'0') as u32;
                    has_digit = true;
                    j += 1;
                }
                if has_digit && j < data.len() && (data[j] == b'h' || data[j] == b'l') {
                    if num == 1 {
                        APP_CURSOR_KEYS.store(data[j] == b'h', Ordering::SeqCst);
                    }
                    i = j + 1;
                    continue;
                }
            }
            if i + 3 < data.len() && data[i + 2] == b'5' && data[i + 3] == b'n' {
                need_status = true;
                i += 4;
                continue;
            }
        }
        i += 1;
    }
    if need_status {
        vt_stdin(vt).push_bytes(b"\x1b[0n");
    }
}

/// Per-VT stdout/stderr endpoint.
pub struct Stdout {
    /// Index of the virtual terminal this stdout belongs to.
    vt: usize,
}

impl INode for Stdin {
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        self.flush_ready_flag();
        let termios = *tty_termios(self.vt).lock();
        let is_canon = termios.c_lflag & ICANON != 0;
        let vmin = termios.c_cc[VMIN];
        let vtime = termios.c_cc[VTIME];

        let mut stdin_buf = self.buf.lock();
        if stdin_buf.is_empty() {
            // Never leave READABLE latched on an empty queue.
            self.eventbus.lock().clear(Event::READABLE);
            if self.eof_pending.swap(false, Ordering::AcqRel) {
                return Ok(0);
            }
            if !is_canon {
                // POSIX non-canonical VMIN/VTIME.
                if vmin == 0 && vtime == 0 {
                    return Ok(0);
                }
                if vmin == 0 && vtime > 0 {
                    let now = kernel_hal::timer::timer_now().as_nanos() as u64;
                    let dl = self.vtime_deadline_ns.load(Ordering::Acquire);
                    if dl == 0 {
                        // VTIME is in deciseconds.
                        let new_dl = now.saturating_add((vtime as u64) * 100_000_000);
                        self.vtime_deadline_ns.store(new_dl, Ordering::Release);
                    } else if now >= dl {
                        self.vtime_deadline_ns.store(0, Ordering::Release);
                        return Ok(0);
                    }
                }
            }
            return Err(FsError::Again);
        }

        // Non-canonical VMIN>1: wait until at least VMIN bytes are buffered
        // (or the user buffer is smaller — then wait for that many).
        if !is_canon && vmin > 1 {
            let need = (vmin as usize).min(buf.len()).max(1);
            if stdin_buf.len() < need {
                return Err(FsError::Again);
            }
        }

        self.vtime_deadline_ns.store(0, Ordering::Release);
        let mut read_bytes = 0;
        while read_bytes < buf.len() && !stdin_buf.is_empty() {
            let ch = stdin_buf.pop_front().unwrap();
            buf[read_bytes] = ch as u8;
            read_bytes += 1;
            if is_canon && ch == '\n' {
                break;
            }
            if !is_canon && vmin > 1 && read_bytes >= vmin as usize {
                break;
            }
        }
        if stdin_buf.is_empty() {
            self.eventbus.lock().clear(Event::READABLE);
        }
        Ok(read_bytes)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        tty_handle_outgoing(self.vt, buf);
        tty_write_out(self.vt, buf);
        Ok(buf.len())
    }

    fn poll(&self) -> Result<PollStatus> {
        self.flush_ready_flag();
        Ok(PollStatus {
            read: self.read_ready(),
            // VT nodes are RDWR (`/dev/ttyN`); Linux always reports POLLOUT.
            write: true,
            error: false,
        })
    }

    fn async_poll<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<PollStatus>> + Send + Sync + 'a>> {
        /// Parks on the stdin eventbus (+ interactive IO wait). Must unsubscribe
        /// on Ready/`Drop`: every poll/epoll re-scan boxes a fresh future and
        /// drops it while Pending — without cleanup each pass leaves an orphan
        /// waker (UAF when a later keystroke fires into freed task memory).
        #[must_use = "future does nothing unless polled/`await`-ed"]
        struct SerialFuture<'a> {
            stdin: &'a Stdin,
            armed: bool,
            sub_id: Option<u64>,
            timer: Option<kernel_hal::timer_waker::TimerWakerSlot>,
            io_waker: Option<core::task::Waker>,
        }

        impl Drop for SerialFuture<'_> {
            fn drop(&mut self) {
                if let Some(id) = self.sub_id.take() {
                    self.stdin.eventbus.lock().unsubscribe(id);
                }
                kernel_hal::timer_waker::kill_timer_waker(&mut self.timer);
                if let Some(w) = self.io_waker.take() {
                    crate::net::clear_io_wait_wakers(&w, false, true);
                }
            }
        }

        impl<'a> Future for SerialFuture<'a> {
            type Output = Result<PollStatus>;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                let this = self.as_mut().get_mut();
                this.stdin.flush_ready_flag();

                // Arm / refresh VTIME deadline for non-canonical VMIN=0 waits.
                {
                    let termios = tty_termios(this.stdin.vt).lock();
                    let noncanon = termios.c_lflag & ICANON == 0;
                    let vmin = termios.c_cc[VMIN];
                    let vtime = termios.c_cc[VTIME];
                    drop(termios);
                    if noncanon && vmin == 0 && vtime > 0 && !this.stdin.can_read() {
                        if this.stdin.vtime_deadline_ns.load(Ordering::Acquire) == 0 {
                            let now = kernel_hal::timer::timer_now().as_nanos() as u64;
                            let new_dl = now.saturating_add((vtime as u64) * 100_000_000);
                            this.stdin
                                .vtime_deadline_ns
                                .store(new_dl, Ordering::Release);
                        }
                    }
                }

                if this.stdin.read_ready() {
                    if let Some(id) = this.sub_id.take() {
                        this.stdin.eventbus.lock().unsubscribe(id);
                    }
                    kernel_hal::timer_waker::kill_timer_waker(&mut this.timer);
                    if let Some(w) = this.io_waker.take() {
                        crate::net::clear_io_wait_wakers(&w, false, true);
                    }
                    return Poll::Ready(Ok(PollStatus {
                        read: true,
                        write: true,
                        error: false,
                    }));
                }

                if this.armed {
                    crate::net::retain_io_wait_wakers(cx.waker(), false, true);
                    this.armed = false;
                } else {
                    crate::net::register_io_wait_wakers(cx.waker(), false, true);
                    this.io_waker = Some(cx.waker().clone());
                    if this.sub_id.is_none() {
                        let waker = cx.waker().clone();
                        this.sub_id = this.stdin.eventbus.lock().subscribe(Box::new(move |_| {
                            waker.wake_by_ref();
                            true
                        }));
                    }
                    // Cancellable VTIME timer so `dd`/`read` with min 0 time N
                    // return after N deciseconds instead of blocking forever.
                    let dl_ns = this.stdin.vtime_deadline_ns.load(Ordering::Acquire);
                    if dl_ns != 0 {
                        kernel_hal::timer_waker::ensure_timer_waker(
                            &mut this.timer,
                            Duration::from_nanos(dl_ns),
                            cx,
                        );
                    }
                    this.armed = true;
                }

                // Poll xHCI from read() path (does not go through poll(2)).
                crate::net::io_wait_tick(false, true);

                this.stdin.flush_ready_flag();
                if this.stdin.read_ready() {
                    if let Some(id) = this.sub_id.take() {
                        this.stdin.eventbus.lock().unsubscribe(id);
                    }
                    kernel_hal::timer_waker::kill_timer_waker(&mut this.timer);
                    if let Some(w) = this.io_waker.take() {
                        crate::net::clear_io_wait_wakers(&w, false, true);
                    }
                    Poll::Ready(Ok(PollStatus {
                        read: true,
                        write: true,
                        error: false,
                    }))
                } else {
                    Poll::Pending
                }
            }
        }

        Box::pin(SerialFuture {
            stdin: self,
            armed: false,
            sub_id: None,
            timer: None,
            io_waker: None,
        })
    }

    fn io_control(&self, cmd: u32, data: usize) -> Result<usize> {
        tty_ioctl(self.vt, cmd, data)
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    /// Get metadata of the INode
    fn metadata(&self) -> Result<Metadata> {
        // Linux VT nodes: `/dev/ttyN` is major 4, minor N. `/dev/tty` (current)
        // uses major 5 minor 0 — that node is the shared `STDIN` alias; per-VT
        // nodes here use (4, vt+1) so `ttyname()` can tell them apart.
        Ok(Metadata {
            dev: 1,
            inode: 100 + self.vt,
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
            rdev: make_rdev(4, self.vt + 1),
        })
    }
}

impl INode for Stdout {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        unimplemented!()
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        tty_handle_outgoing(self.vt, buf);
        tty_write_out(self.vt, buf);
        Ok(buf.len())
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: false,
            write: true,
            error: false,
        })
    }

    fn io_control(&self, cmd: u32, data: usize) -> Result<usize> {
        tty_ioctl(self.vt, cmd, data)
    }

    /// Get metadata of the INode
    fn metadata(&self) -> Result<Metadata> {
        // Same (dev, inode) as `Stdin` for this VT so `ttyname()` on fd 1/2
        // matches `stat("/dev/ttyN")`.
        Ok(Metadata {
            dev: 1,
            inode: 100 + self.vt,
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
            rdev: make_rdev(4, self.vt + 1),
        })
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}
