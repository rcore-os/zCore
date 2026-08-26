//! Pseudo-terminals (`/dev/ptmx` + `/dev/pts/N`).
//!
//! A PTY is a bidirectional pipe with a terminal line discipline in the middle:
//!
//! ```text
//!   terminal (st)  ──write──►  master  ──[input discipline]──►  slave read   (shell stdin)
//!   terminal (st)  ◄──read───  master  ◄──[output discipline]──  slave write  (shell stdout)
//! ```
//!
//! Opening `/dev/ptmx` allocates a fresh pair and exposes the slave at
//! `/dev/pts/N`; the master is returned to the opener. This is what lets a
//! terminal emulator run a real shell under TinyX/Xfbdev.
//!
//! Only the common path is implemented (canonical + raw input, ECHO, ISIG,
//! ICRNL/ONLCR, winsize, the ptmx/pts ioctls). It is deliberately gated behind
//! `/dev/ptmx`, so nothing else in the system is affected.

use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};
use core::{
    any::Any,
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll},
};

use kernel_hal::console::ConsoleWinSize;
use lock::Mutex;
use rcore_fs::vfs::*;
use rcore_fs_devfs::DevFS;

use super::super::ioctl::*;
use crate::fs::stdio::wake_tty_intr_waiters;

// c_iflag
const INLCR: u32 = 0x0040;
const IGNCR: u32 = 0x0080;
const ICRNL: u32 = 0x0100;
// c_oflag
const OPOST: u32 = 0x0001;
const ONLCR: u32 = 0x0004;
// c_lflag
const ISIG: u32 = 0x0001;
const ICANON: u32 = 0x0002;
const ECHO: u32 = 0x0008;
const ECHOE: u32 = 0x0010;
// c_cc indices
const VINTR: usize = 0;
const VQUIT: usize = 1;
const VERASE: usize = 2;
const VEOF: usize = 4;

const PTY_BUF_CAP: usize = 16 * 1024;

/// Shared state of one PTY pair.
struct PtyInner {
    /// slave → master: program output, read by the terminal.
    output: VecDeque<u8>,
    /// master → slave (after the input line discipline): read by the program.
    input: VecDeque<u8>,
    /// In-progress canonical line, not yet visible to the slave reader.
    canon: VecDeque<u8>,
    termios: Termios,
    winsize: ConsoleWinSize,
    /// Foreground process group of the slave (`TIOCSPGRP`), for job control
    /// and signal delivery (Ctrl-C).
    fg_pgrp: i32,
    /// `TIOCSPTLCK` lock flag (1 = locked). The terminal clears it via
    /// `unlockpt()` before opening the slave.
    locked: i32,
    num: u32,
    master_open: bool,
}

impl PtyInner {
    fn new(num: u32) -> Self {
        Self {
            output: VecDeque::new(),
            input: VecDeque::new(),
            canon: VecDeque::new(),
            termios: Termios::default_tty(),
            winsize: ConsoleWinSize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
            fg_pgrp: 0,
            locked: 1,
            num,
            master_open: true,
        }
    }

    fn push_output(&mut self, b: u8) {
        if self.output.len() < PTY_BUF_CAP {
            self.output.push_back(b);
        }
    }

    /// Process one byte written to the master (a keystroke from the terminal)
    /// through the slave's input line discipline.
    fn master_input_byte(&mut self, mut b: u8) {
        let iflag = self.termios.c_iflag;
        let lflag = self.termios.c_lflag;
        let cc = self.termios.c_cc;

        // CR/NL input translation.
        if b == b'\r' {
            if iflag & IGNCR != 0 {
                return;
            }
            if iflag & ICRNL != 0 {
                b = b'\n';
            }
        } else if b == b'\n' && iflag & INLCR != 0 {
            b = b'\r';
        }

        // Signals (Ctrl-C / Ctrl-\).
        if lflag & ISIG != 0 && (b == cc[VINTR] || b == cc[VQUIT]) {
            let sig = if b == cc[VINTR] {
                crate::signal::Signal::SIGINT
            } else {
                crate::signal::Signal::SIGQUIT
            };
            if lflag & ECHO != 0 {
                self.echo_ctrl(b);
            }
            if self.fg_pgrp > 0 {
                let _ = crate::process::send_signal_to_process(self.fg_pgrp as usize, sig);
            }
            return;
        }

        if lflag & ICANON != 0 {
            // Erase (Backspace / DEL).
            if b == cc[VERASE] {
                if self.canon.pop_back().is_some() && lflag & (ECHO | ECHOE) != 0 {
                    // Erase the echoed glyph: backspace, space, backspace.
                    self.push_output(0x08);
                    self.push_output(b' ');
                    self.push_output(0x08);
                }
                return;
            }
            // End of file on an empty line: deliver a zero-length read.
            if b == cc[VEOF] {
                self.commit_canon();
                return;
            }
            if lflag & ECHO != 0 {
                if b == b'\n' {
                    self.push_output(b'\n');
                } else {
                    self.echo_ctrl(b);
                }
            }
            self.canon.push_back(b);
            if b == b'\n' {
                self.commit_canon();
            }
        } else {
            // Raw mode: deliver immediately.
            if lflag & ECHO != 0 {
                self.echo_ctrl(b);
            }
            if self.input.len() < PTY_BUF_CAP {
                self.input.push_back(b);
            }
        }
    }

    /// Echo a byte to the master, rendering control chars as `^X`.
    fn echo_ctrl(&mut self, b: u8) {
        if (b < 0x20 && b != b'\n' && b != b'\t') || b == 0x7f {
            self.push_output(b'^');
            self.push_output(if b == 0x7f { b'?' } else { b'@' + b });
        } else {
            self.push_output(b);
        }
    }

    fn commit_canon(&mut self) {
        while let Some(b) = self.canon.pop_front() {
            if self.input.len() < PTY_BUF_CAP {
                self.input.push_back(b);
            }
        }
    }

    /// Process bytes written by the slave (program output) toward the master,
    /// applying `OPOST`/`ONLCR` (NL → CR-NL).
    fn slave_output(&mut self, buf: &[u8]) {
        let opost = self.termios.c_oflag & OPOST != 0;
        let onlcr = self.termios.c_oflag & ONLCR != 0;
        for &b in buf {
            if opost && onlcr && b == b'\n' {
                self.push_output(b'\r');
            }
            self.push_output(b);
        }
    }
}

// ----------------------------------------------------------------------------
// Registry: pts number → slave inode, plus the `/dev/pts` directory handle.
// ----------------------------------------------------------------------------

lazy_static::lazy_static! {
    static ref PTYS: Mutex<BTreeMap<u32, Arc<PtySlave>>> = Mutex::new(BTreeMap::new());
}
static NEXT_PTY: AtomicU32 = AtomicU32::new(0);

/// `/dev/pts`: a directory whose children resolve dynamically to live slaves.
/// `ptsname()` opens `/dev/pts/N`; lookup walks here and `find("N")` returns the
/// registered slave for pty number `N`.
pub struct PtsDir {
    inode_id: usize,
}

impl Default for PtsDir {
    fn default() -> Self {
        Self::new()
    }
}

impl PtsDir {
    pub fn new() -> Self {
        Self {
            inode_id: DevFS::new_inode_id(),
        }
    }
}

impl INode for PtsDir {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Err(FsError::IsDir)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::IsDir)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        let num: u32 = name.parse().map_err(|_| FsError::EntryNotFound)?;
        PTYS.lock()
            .get(&num)
            .cloned()
            .map(|s| s as Arc<dyn INode>)
            .ok_or(FsError::EntryNotFound)
    }
    fn metadata(&self) -> Result<Metadata> {
        let mut m = chardev_metadata(self.inode_id, 0o755);
        m.type_ = FileType::Dir;
        Ok(m)
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}

// ----------------------------------------------------------------------------
// `/dev/ptmx`: opening it clones a new pair.
// ----------------------------------------------------------------------------

/// The `/dev/ptmx` node. Resolving it is normal; the open path downcasts to
/// this type and calls [`PtmxINode::open_master`] to get a fresh master.
pub struct PtmxINode {
    inode_id: usize,
}

impl Default for PtmxINode {
    fn default() -> Self {
        Self::new()
    }
}

impl PtmxINode {
    pub fn new() -> Self {
        Self {
            inode_id: DevFS::new_inode_id(),
        }
    }

    /// Allocate a new PTY pair: publish the slave at `/dev/pts/N` and return
    /// the master inode for the opener.
    pub fn open_master(&self) -> Result<Arc<dyn INode>> {
        let num = NEXT_PTY.fetch_add(1, Ordering::Relaxed);
        let inner = Arc::new(Mutex::new(PtyInner::new(num)));
        let slave = Arc::new(PtySlave {
            inner: inner.clone(),
            inode_id: DevFS::new_inode_id(),
        });
        let master = Arc::new(PtyMaster {
            inner,
            inode_id: DevFS::new_inode_id(),
        });
        PTYS.lock().insert(num, slave);
        Ok(master as Arc<dyn INode>)
    }
}

impl INode for PtmxINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: false,
            write: true,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(chardev_metadata(self.inode_id, 0o666))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}

// ----------------------------------------------------------------------------
// Master end.
// ----------------------------------------------------------------------------

pub struct PtyMaster {
    inner: Arc<Mutex<PtyInner>>,
    inode_id: usize,
}

impl Drop for PtyMaster {
    fn drop(&mut self) {
        let num = {
            let mut g = self.inner.lock();
            g.master_open = false;
            g.num
        };
        PTYS.lock().remove(&num);
        wake_tty_intr_waiters();
    }
}

impl INode for PtyMaster {
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        let mut g = self.inner.lock();
        if g.output.is_empty() {
            // Block until the program produces output. The master never EOFs on
            // an idle slave (slaves may not have been opened yet); the terminal
            // detects the child exiting via waitpid and closes the master.
            return Err(FsError::Again);
        }
        let mut n = 0;
        while n < buf.len() {
            match g.output.pop_front() {
                Some(b) => {
                    buf[n] = b;
                    n += 1;
                }
                None => break,
            }
        }
        Ok(n)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        {
            let mut g = self.inner.lock();
            for &b in buf {
                g.master_input_byte(b);
            }
        }
        wake_tty_intr_waiters();
        Ok(buf.len())
    }

    fn poll(&self) -> Result<PollStatus> {
        let g = self.inner.lock();
        Ok(PollStatus {
            read: !g.output.is_empty(),
            write: true,
            error: false,
        })
    }

    fn async_poll<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<PollStatus>> + Send + Sync + 'a>> {
        Box::pin(PtyReadFuture {
            inner: &self.inner,
            master: true,
            armed: false,
        })
    }

    fn io_control(&self, cmd: u32, data: usize) -> Result<usize> {
        pty_ioctl(&self.inner, true, cmd, data)
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(chardev_metadata(self.inode_id, 0o600))
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}

// ----------------------------------------------------------------------------
// Slave end (`/dev/pts/N`).
// ----------------------------------------------------------------------------

pub struct PtySlave {
    inner: Arc<Mutex<PtyInner>>,
    inode_id: usize,
}

impl INode for PtySlave {
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        let mut g = self.inner.lock();
        if g.input.is_empty() {
            if !g.master_open {
                return Ok(0); // EOF: master closed
            }
            return Err(FsError::Again);
        }
        let mut n = 0;
        while n < buf.len() {
            match g.input.pop_front() {
                Some(b) => {
                    buf[n] = b;
                    n += 1;
                }
                None => break,
            }
        }
        Ok(n)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        {
            let mut g = self.inner.lock();
            g.slave_output(buf);
        }
        wake_tty_intr_waiters();
        Ok(buf.len())
    }

    fn poll(&self) -> Result<PollStatus> {
        let g = self.inner.lock();
        Ok(PollStatus {
            read: !g.input.is_empty() || !g.master_open,
            write: true,
            error: false,
        })
    }

    fn async_poll<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<PollStatus>> + Send + Sync + 'a>> {
        Box::pin(PtyReadFuture {
            inner: &self.inner,
            master: false,
            armed: false,
        })
    }

    fn io_control(&self, cmd: u32, data: usize) -> Result<usize> {
        pty_ioctl(&self.inner, false, cmd, data)
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(chardev_metadata(self.inode_id, 0o620))
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}

// ----------------------------------------------------------------------------
// Shared ioctl handling.
// ----------------------------------------------------------------------------

const TIOCSPTLCK: u32 = 0x4004_5431; // set/clear pty lock
const TIOCGPTN: u32 = 0x8004_5430; // get pty number

fn pty_ioctl(inner: &Arc<Mutex<PtyInner>>, master: bool, cmd: u32, data: usize) -> Result<usize> {
    let cmd = cmd as usize;
    let mut g = inner.lock();
    match cmd {
        TCGETS => {
            if data == 0 {
                return Err(FsError::InvalidParam);
            }
            unsafe { *(data as *mut Termios) = g.termios };
            Ok(0)
        }
        TCSETS | TCSETSW | TCSETSF => {
            if data == 0 {
                return Err(FsError::InvalidParam);
            }
            g.termios = unsafe { *(data as *const Termios) };
            Ok(0)
        }
        TIOCGWINSZ => {
            if data == 0 {
                return Err(FsError::InvalidParam);
            }
            unsafe { *(data as *mut ConsoleWinSize) = g.winsize };
            Ok(0)
        }
        TIOCSWINSZ => {
            if data == 0 {
                return Err(FsError::InvalidParam);
            }
            g.winsize = unsafe { *(data as *const ConsoleWinSize) };
            Ok(0)
        }
        TIOCSPGRP => {
            if data != 0 {
                g.fg_pgrp = unsafe { *(data as *const i32) };
            }
            Ok(0)
        }
        TIOCGPGRP => {
            if data == 0 {
                return Err(FsError::InvalidParam);
            }
            let mut pgid = g.fg_pgrp;
            if pgid == 0 {
                // Same caller-pgrp fallback as `fs::pty` / stdio: reporting 0
                // (or any constant) to busybox ash's job-control init leaves it
                // looping `killpg(0, SIGTTIN)` forever with no prompt, because
                // clients that acquire the ctty implicitly (setsid + first
                // slave open, e.g. xterm) never issue TIOCSCTTY to seed us.
                use zircon_object::object::KernelObject;
                if let Some(arc) = kernel_hal::thread::get_current_thread() {
                    if let Ok(thread) = arc.downcast::<zircon_object::task::Thread>() {
                        pgid = crate::process::get_process_pgid(thread.proc().id()).unwrap_or(0)
                            as i32;
                    }
                }
                if pgid == 0 {
                    pgid = 1;
                }
            }
            unsafe { *(data as *mut i32) = pgid };
            Ok(0)
        }
        _ if cmd as u32 == TIOCGPTN => {
            if !master || data == 0 {
                return Err(FsError::InvalidParam);
            }
            unsafe { *(data as *mut u32) = g.num };
            Ok(0)
        }
        _ if cmd as u32 == TIOCSPTLCK => {
            if !master || data == 0 {
                return Err(FsError::InvalidParam);
            }
            g.locked = unsafe { *(data as *const i32) };
            Ok(0)
        }
        // TIOCSCTTY / TIOCNOTTY: accept (controlling-tty assignment is a no-op
        // here); TCFLSH: nothing buffered worth flushing distinctly — accept
        // rather than hand terminals a spurious ENOTTY.
        0x540E | TIOCNOTTY | TCFLSH => Ok(0),
        _ => Err(FsError::NotSupported),
    }
}

// ----------------------------------------------------------------------------
// Blocking-read future: wakes when the peer writes (via the TTY intr wakers).
// ----------------------------------------------------------------------------

struct PtyReadFuture<'a> {
    inner: &'a Arc<Mutex<PtyInner>>,
    master: bool,
    armed: bool,
}

impl<'a> Future for PtyReadFuture<'a> {
    type Output = Result<PollStatus>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let this = self.as_mut().get_mut();
        let ready = {
            let g = this.inner.lock();
            if this.master {
                !g.output.is_empty()
            } else {
                !g.input.is_empty() || !g.master_open
            }
        };
        if ready {
            return Poll::Ready(Ok(PollStatus {
                read: true,
                write: true,
                error: false,
            }));
        }
        if this.armed {
            crate::net::retain_io_wait_wakers(cx.waker(), false, true);
        } else {
            crate::net::register_io_wait_wakers(cx.waker(), false, true);
            this.armed = true;
        }
        Poll::Pending
    }
}

// ----------------------------------------------------------------------------

fn chardev_metadata(inode_id: usize, mode: u16) -> Metadata {
    Metadata {
        dev: 0,
        inode: inode_id,
        size: 0,
        blk_size: 0,
        blocks: 0,
        atime: Timespec { sec: 0, nsec: 0 },
        mtime: Timespec { sec: 0, nsec: 0 },
        ctime: Timespec { sec: 0, nsec: 0 },
        type_: FileType::CharDevice,
        mode,
        nlinks: 1,
        uid: 0,
        gid: 0,
        rdev: 0,
    }
}
