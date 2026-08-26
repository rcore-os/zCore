#![allow(unused)]

// for IOR and IOW:
// 32bits total, command in lower 16bits, size of the parameter structure in the lower 14 bits of the upper 16 bits
// higher 2 bits: 01 = write, 10 = read

#[cfg(not(target_arch = "mips"))]
pub const TCGETS: usize = 0x5401;
#[cfg(target_arch = "mips")]
pub const TCGETS: usize = 0x540D;

pub const TCSETS: usize = 0x5402;
pub const TCSETSW: usize = 0x5403;
pub const TCSETSF: usize = 0x5404;

/// The Linux **kernel ABI** `struct termios` exchanged by `TCGETS`/`TCSETS`
/// (asm-generic/termbits.h): `NCCS = 19`, no separate `c_ispeed`/`c_ospeed`
/// (the line speed lives in the `CBAUD` bits of `c_cflag`). This is exactly 36
/// bytes.
///
/// This MUST match the kernel ABI struct, not libc's larger userspace `struct
/// termios` (60 bytes, `NCCS = 32` + speed fields). glibc's `tcgetattr` passes
/// a kernel-sized stack buffer to `TCGETS` and expects ≤36 bytes back; writing
/// 60 bytes overran that buffer, clobbering the saved frame pointer and return
/// address and crashing the process on return (SIGSEGV, `pc=0xf`) — which made
/// every glibc `exec` flaky. musl passes its own 60-byte struct, so it happened
/// to survive, masking the bug.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; 19],
}

impl Termios {
    /// Defaults aligned with Linux `n_tty` cooked TTY settings.
    pub const fn default_tty() -> Self {
        Self {
            // ICRNL | IXON | IMAXBEL
            c_iflag: 0x2500,
            // OPOST | ONLCR
            c_oflag: 0x0005,
            // B38400 | CS8 | CREAD | HUPCL
            c_cflag: 0x08bf,
            // ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN
            c_lflag: 0x803b,
            c_line: 0,
            // Matches Linux `INIT_C_CC` (include/linux/tty.h), one entry per
            // NCCS=19 control char: VINTR=^C, VQUIT=^\, VERASE=DEL, VKILL=^U,
            // VEOF=^D, VTIME=0, VMIN=1, VSWTC=0, VSTART=^Q, VSTOP=^S, VSUSP=^Z,
            // VEOL=0, VREPRINT=^R, VDISCARD=^O, VWERASE=^W, VLNEXT=^V, VEOL2=0.
            c_cc: [
                3, 28, 127, 21, 4, 0, 1, 0, 17, 19, 26, 0, 18, 15, 23, 22, 0, 0, 0,
            ],
            // Line speed is carried in the CBAUD bits of c_cflag (B38400 above).
        }
    }
}

#[cfg(not(target_arch = "mips"))]
pub const TIOCGPGRP: usize = 0x540F;
// _IOR('t', 119, int)
#[cfg(target_arch = "mips")]
pub const TIOCGPGRP: usize = 0x4_004_74_77;

#[cfg(not(target_arch = "mips"))]
pub const TIOCSPGRP: usize = 0x5410;
// _IOW('t', 118, int)
#[cfg(target_arch = "mips")]
pub const TIOCSPGRP: usize = 0x8_004_74_76;

#[cfg(not(target_arch = "mips"))]
pub const TIOCGWINSZ: usize = 0x5413;
// _IOR('t', 104, struct winsize)
#[cfg(target_arch = "mips")]
pub const TIOCGWINSZ: usize = 0x4_008_74_68;

#[cfg(not(target_arch = "mips"))]
pub const TIOCSWINSZ: usize = 0x5414;
// _IOW('t', 103, struct winsize)
#[cfg(target_arch = "mips")]
pub const TIOCSWINSZ: usize = 0x8_008_74_67;

/// Linux-specific console multiplexor ioctl; the subcommand is the first byte
/// of the argument (e.g. 6 = `TIOCL_GETSHIFTSTATE`).
pub const TIOCLINUX: usize = 0x541c;
/// `TIOCLINUX` subcommand: read the keyboard shift/modifier state.
pub const TIOCL_GETSHIFTSTATE: u8 = 6;

#[cfg(not(target_arch = "mips"))]
pub const FIONCLEX: usize = 0x5450;
#[cfg(target_arch = "mips")]
pub const FIONCLEX: usize = 0x6602;

#[cfg(not(target_arch = "mips"))]
pub const FIOCLEX: usize = 0x5451;
#[cfg(target_arch = "mips")]
pub const FIOCLEX: usize = 0x6601;

// rustc using pipe and ioctl pipe file with this request id
// for non-blocking/blocking IO control setting
pub const FIONBIO: usize = 0x5421;

// Queue / session ioctls (`<asm-generic/ioctls.h>`).
/// Bytes available to read (a.k.a. `TIOCINQ`); written as an `int`.
pub const FIONREAD: usize = 0x541B;
/// Alias of [`FIONREAD`] — bytes waiting in the TTY input queue.
pub const TIOCINQ: usize = FIONREAD;
/// Bytes still queued in the TTY output buffer; written as an `int`.
pub const TIOCOUTQ: usize = 0x5411;
/// Get the session ID of the terminal; written as a `pid_t` (`int`).
pub const TIOCGSID: usize = 0x5429;
/// Get serial line interrupt counters into a [`SerialIcounter`].
pub const TIOCGICOUNT: usize = 0x545D;

/// Linux `struct serial_icounter_struct` — cumulative serial line event
/// counters reported by `TIOCGICOUNT`. Virtual TTYs have no real UART, so all
/// fields are reported as zero.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SerialIcounter {
    pub cts: i32,
    pub dsr: i32,
    pub rng: i32,
    pub dcd: i32,
    pub rx: i32,
    pub tx: i32,
    pub frame: i32,
    pub overrun: i32,
    pub parity: i32,
    pub brk: i32,
    pub buf_overrun: i32,
    pub reserved: [i32; 9],
}

// Modem control line ioctls (`<asm-generic/termios.h>`). The argument is an
// `int` bitmask of the `TIOCM_*` flags below.
/// Read the state of the modem control lines into an `int`.
pub const TIOCMGET: usize = 0x5415;
/// Set the modem control lines to the given bitmask.
pub const TIOCMSET: usize = 0x5418;
/// Set (OR in) the given modem control line bits.
pub const TIOCMBIS: usize = 0x5416;
/// Clear (AND out) the given modem control line bits.
pub const TIOCMBIC: usize = 0x5417;

/// DTR (Data Terminal Ready) output line.
pub const TIOCM_DTR: i32 = 0x002;
/// RTS (Request To Send) output line.
pub const TIOCM_RTS: i32 = 0x004;
/// CTS (Clear To Send) input line.
pub const TIOCM_CTS: i32 = 0x020;
/// Carrier Detect input line (a.k.a. `TIOCM_CD`).
pub const TIOCM_CAR: i32 = 0x040;
/// DSR (Data Set Ready) input line.
pub const TIOCM_DSR: i32 = 0x100;

// VT / KD console ioctls (Linux `<linux/kd.h>`).
/// Get console mode (`KD_TEXT` / `KD_GRAPHICS`) into an `int`.
pub const KDGETMODE: usize = 0x4B3B;
/// Set console mode from an `int` (`KD_TEXT` / `KD_GRAPHICS`).
pub const KDSETMODE: usize = 0x4B3A;
/// Text mode: the kernel draws the framebuffer console.
pub const KD_TEXT: usize = 0x00;
/// Graphics mode: userspace owns the framebuffer; the console stops drawing.
pub const KD_GRAPHICS: usize = 0x01;

/// Get keyboard type (`<linux/kd.h>`), written as a single `char`. Used by X to
/// validate that a file descriptor is really a virtual console.
pub const KDGKBTYPE: usize = 0x4B33;
/// 101-key PC keyboard — the value reported by `KDGKBTYPE`.
pub const KB_101: u8 = 0x02;

/// Get keyboard translation mode (`K_RAW` / `K_XLATE` / ...) into an `int`.
pub const KDGKBMODE: usize = 0x4B44;
/// Set keyboard translation mode from an `int`.
pub const KDSKBMODE: usize = 0x4B45;
/// Raw scancodes; the kernel does no translation.
pub const K_RAW: i32 = 0x00;
/// Cooked mode: keycodes translated to characters (the default).
pub const K_XLATE: i32 = 0x01;
/// Medium-raw keycodes.
pub const K_MEDIUMRAW: i32 = 0x02;
/// Unicode translation.
pub const K_UNICODE: i32 = 0x03;
/// Keyboard input disabled — used by X/Wayland while they own input via evdev.
pub const K_OFF: i32 = 0x04;

// Virtual terminal ioctls (Linux `<linux/vt.h>`).
/// Find the first free VT number; writes a 1-based VT index into an `int`.
pub const VT_OPENQRY: usize = 0x5600;
/// Get the VT switching mode into a [`VtMode`].
pub const VT_GETMODE: usize = 0x5601;
/// Set the VT switching mode from a [`VtMode`].
pub const VT_SETMODE: usize = 0x5602;
/// Get global VT state into a [`VtStat`].
pub const VT_GETSTATE: usize = 0x5603;
/// Acknowledge a VT release/acquire (arg by value).
pub const VT_RELDISP: usize = 0x5605;
/// Make the given (1-based) VT active (arg by value).
pub const VT_ACTIVATE: usize = 0x5606;
/// Wait until the given (1-based) VT is active (arg by value).
pub const VT_WAITACTIVE: usize = 0x5607;
/// Deallocate the given VT (arg by value).
pub const VT_DISALLOCATE: usize = 0x5608;

/// `mode` value: kernel handles VT switches automatically (default).
pub const VT_AUTO: u8 = 0x00;
/// `mode` value: the process handles VT switches via signals.
pub const VT_PROCESS: u8 = 0x01;

/// Linux `struct vt_mode` — VT switch signalling configuration.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VtMode {
    pub mode: u8,
    pub waitv: u8,
    pub relsig: i16,
    pub acqsig: i16,
    pub frsig: i16,
}

impl VtMode {
    /// Default mode: automatic, kernel-driven VT switching.
    pub const fn auto() -> Self {
        Self {
            mode: VT_AUTO,
            waitv: 0,
            relsig: 0,
            acqsig: 0,
            frsig: 0,
        }
    }
}

/// Linux `struct vt_stat` — global VT state returned by `VT_GETSTATE`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VtStat {
    pub v_active: u16,
    pub v_signal: u16,
    pub v_state: u16,
}

// Misc TTY control ioctls an X server may issue; accepted as no-ops.
/// Flush the terminal queues.
pub const TCFLSH: usize = 0x540B;
/// Make the terminal the controlling TTY of the calling process.
pub const TIOCSCTTY: usize = 0x540E;
/// Give up the controlling TTY.
pub const TIOCNOTTY: usize = 0x5422;

// Pseudo-terminal (PTY) ioctls issued on the `/dev/ptmx` master.
/// Get the PTY number of the master (`unsigned int` out) — `ptsname(3)` uses it
/// to build `/dev/pts/N`.
pub const TIOCGPTN: usize = 0x8004_5430;
/// Lock/unlock the PTY slave (`int` in); `unlockpt(3)` writes 0 here.
pub const TIOCSPTLCK: usize = 0x4004_5431;
/// Open the slave side of the master without a path (returns an fd). Accepted
/// best-effort; most libcs fall back to `open("/dev/pts/N")`.
pub const TIOCGPTPEER: usize = 0x5441;

/// Get keyboard LED state (Scroll/Num/Caps) into an `int`.
pub const KDGETLED: usize = 0x4B11;
/// Set keyboard LED state from an `int` (by value, not a pointer).
pub const KDSETLED: usize = 0x4B32;
/// Read one keymap entry into a [`KbEntry`].
pub const KDGKBENT: usize = 0x4B46;
/// Console bell tone (accepted as a no-op).
pub const KDMKTONE: usize = 0x4B30;

/// Key-type field in a `kb_value` (`KTYP(x) == (x >> 8)`).
pub const KT_LATIN: u16 = 0;
pub const KT_FN: u16 = 1;
pub const KT_SPEC: u16 = 2;

/// Linux `struct kbentry` — one keymap cell for `KDGKBENT`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct KbEntry {
    pub kb_table: u8,
    pub kb_index: u8,
    pub kb_value: u16,
}
