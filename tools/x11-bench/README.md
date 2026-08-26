# X11 kernel-interaction bench

A QEMU smoke test for the kernel surface an X server (`startx`) uses on Eclipse
OS (zCore). It boots zCore via UEFI with a tiny static C init (`xtest.c`) that
drives, in one real boot, every primitive Xorg relies on:

- **Framebuffer** — `open("/dev/fb0")`, `FBIOGET_VSCREENINFO`, `mmap`, draw.
- **Input (evdev)** — `open("/dev/input/event0")`, `EVIOCGNAME`, `EVIOCGBIT`.
- **VT graphics handoff** — the `xf86OpenConsole`/kdrive `LinuxInit` sequence:
  `VT_OPENQRY`/`VT_GETSTATE`/`VT_ACTIVATE`/`VT_WAITACTIVE`, then
  `KDSKBMODE(K_RAW)` and `KDSETMODE(KD_GRAPHICS)` (both pass the mode *by value*),
  checking `KDGETMODE` reports `KD_GRAPHICS` back. This is the step that seizes
  the console; mishandling the by-value argument faults the kernel.
- **TTY** — `TIOCGWINSZ` on a pipe (must succeed, not `ENOTTY`).
- **AF_UNIX** — filesystem *and* abstract (`\0/tmp/.X11-unix/Xn`, Xlib's primary
  transport): `bind`/`listen`/`connect`, a client write *before* `accept`, then
  `accept`/`read` (the X11 connection-setup handshake).
- **Event loop** — `select`, `epoll`, `eventfd` reporting a ready socket.
- **Scheduler** — `SIGALRM` via `setitimer`.
- **Input thread** — `pthread_create` + `join`.
- **`socketpair`**, and **xauth's** write-temp + atomic `rename`.

## Run

```sh
tools/x11-bench/run.sh
```

Prints one `XTEST: [PASS|FAIL] <name>` line per check and `BENCH OK` when all
pass. Expected:

```
XTEST: [PASS] fb screeninfo
XTEST: [PASS] fb mmap+draw
XTEST: [PASS] evdev EVIOCGNAME ps2-input
XTEST: [PASS] evdev EVIOCGBIT
XTEST: [PASS] vt graphics handoff
XTEST: [PASS] TIOCGWINSZ on pipe
XTEST: [PASS] unix fs socket
XTEST: [PASS] unix abstract socket
XTEST: [PASS] select
XTEST: [PASS] epoll
XTEST: [PASS] eventfd
XTEST: [PASS] SIGALRM/setitimer
XTEST: [PASS] pthread
XTEST: [PASS] socketpair
XTEST: [PASS] xauth write+rename
XTEST: 15/15 passed
XTEST: done
```

## Requirements

- `qemu-system-x86_64`
- the `x86_64-linux-musl-cross` toolchain under `target/x86_64/`
  (downloaded by `cargo rootfs`)
- `rboot/OVMF.fd`

The complementary host-side unit tests for the AF_UNIX handshake live in
`linux-object/src/net/unix.rs` (`cargo test -p linux-object net::unix`).
