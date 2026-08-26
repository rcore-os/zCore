# zCore

[![CI](https://github.com/rcore-os/zCore/workflows/CI/badge.svg?branch=master)](https://github.com/rcore-os/zCore/actions)
[![Docs](https://img.shields.io/badge/docs-alpha-blue)](https://rcore-os.github.io/zCore/)
[![Coverage Status](https://coveralls.io/repos/github/rcore-os/zCore/badge.svg?branch=master)](https://coveralls.io/github/rcore-os/zCore?branch=master)

Reimplement [Zircon][zircon] microkernel in safe Rust as a userspace program!

## Dev Status

🚧 Working In Progress

- 2020.04.16: Zircon console is working on zCore! 🎉

## Quick start for RISCV64

```sh
make qemu ARCH=riscv64
```

## Getting started

Environments：

- [Rust toolchain](http://rustup.rs)
- [QEMU](https://www.qemu.org)

### Developing environment info

- current rustc -- rustc 1.97.0-nightly
- current rust-toolchain -- nightly-2026-05-01
- current qemu -- 6.0+

Clone repo and initialize dependencies:

```sh
git clone https://github.com/rcore-os/zCore --recursive
cd zCore
cargo update-all
cargo zircon-init
```

For users in China, there's a mirror you can try:

```sh
git clone https://github.com.cnpmjs.org/rcore-os/zCore --recursive
```

Use docker container as standand develop environment, please refer to [tootls/docker](https://github.com/rcore-os/zCore/tree/master/tools/docker).

### Run zcore in libos mode

#### Run zcore in linux-libos mode

- step 1: Prepare Alpine Linux rootfs:

  ```sh
  make rootfs
  ```

- step 2: Compile & Run native Linux program (Busybox) in libos mode:

  ```sh
  cargo linux-libos --args /bin/busybox
  ```

  You can also add the feature `graphic` to show the graphical output (with [sdl2](https://www.libsdl.org) installed).

  To debug, set the `LOG` environment variable to one of `error`, `warn`, `info`, `debug`, `trace`.

#### Run native Zircon program (shell) in zircon-libos mode:

- step 1: Compile and Run Zircon shell

  ```sh
  cargo run --release --features "zircon libos" -- prebuilt/zircon/x64/bringup.zbi
  ```

  The `graphic` and `LOG` options are the same as Linux.

### Run zcore in bare-metal mode

#### Run Linux shell in  linux-bare-metal mode:

- step 1: Prepare Alpine Linux rootfs:

  ```sh
  make rootfs
  ```

- step 2: Create Linux rootfs image:

  Note: Before below step, you can add some special apps in zCore/rootfs

  ```sh
  make image
  ```

- step 3: Build and run zcore in  linux-bare-metal mode:

  ```sh
  cd zCore && make run MODE=release LINUX=1 [LOG=warn] [GRAPHIC=on] [ACCEL=1]
  ```

#### Run Zircon shell in zircon-bare-metal mode:

- step 1: Build and run zcore in  zircon-bare-metal mode:

  ```sh
  cd zCore && make run MODE=release [LOG=warn] [GRAPHIC=on] [ACCEL=1]
  ```

- step 2: Build and run your own Zircon user programs:

  ```sh
  # See template in zircon-user
  cd zircon-user && make zbi MODE=release
  
  # Run your programs in zCore
  cd zCore && make run MODE=release USER=1 [LOG=warn] [GRAPHIC=on] [ACCEL=1]
  ```

## Testing

### LibOS Mode Testing

#### Zircon related

Run Zircon official core-tests:

```sh
pip3 install pexpect
cd scripts && python3 unix-core-testone.py 'Channel.*'
```

Run all (non-panicked) core-tests for CI:

```sh
pip3 install pexpect
cd scripts && python3 unix-core-tests.py
# Check `zircon/test-result.txt` for results.
```

#### Linux related

Run Linux musl libc-tests for CI:

```sh
make rootfs && make libc-test
cd scripts && python3 libos-libc-tests.py
# Check `linux/test-result.txt` for results.
```

### Bare-metal Mode Testing

#### Zircon related

Run Zircon official core-tests on bare-metal:

```sh
cd zCore && make test MODE=release [ACCEL=1] TEST_FILTER='Channel.*'
```

Run all (non-panicked) core-tests for CI:

```sh
pip3 install pexpect
cd scripts && python3 core-tests.py
# Check `zircon/test-result.txt` for results.
```

#### x86-64 Linux related

Run Linux musl libc-tests for CI:

```sh
##  Prepare rootfs with libc-test apps
make baremetal-test-img
## Build zCore kernel
cd zCore && make build MODE=release LINUX=1 ARCH=x86_64
## Testing
cd scripts && python3 baremetal-libc-test.py
##
```

You can use [`scripts/baremetal-libc-test-ones.py`](./scripts/baremetal-libc-test-ones.py) & [`scripts/linux/baremetal-test-ones.txt`](./scripts/linux/baremetal-test-ones.txt) to test specified apps.

[`scripts/linux/baremetal-test-fail.txt`](./scripts/linux/baremetal-test-fail.txt) includes all failed x86-64 apps (We need YOUR HELP to fix bugs!)

#### riscv-64 Linux related

Run Linux musl libc-tests for CI:

```sh
##  Prepare rootfs with libc-test & oscomp apps
make riscv-image
## Build zCore kernel & Testing
cd scripts && python3 baremetal-test-riscv64.py
##
```

You can use[scripts/baremetal-libc-test-ones-riscv64.py](./scripts/baremetal-libc-test-ones-riscv64.py) & [`scripts/linux/baremetal-test-ones-rv64.txt`](scripts/linux/baremetal-test-ones-rv64.txt)to test
specified apps.

[`scripts/linux/baremetal-test-fail-riscv64.txt`](./scripts/linux/baremetal-test-fail-riscv64.txt)includes all failed riscv-64 apps (We need YOUR HELP to fix bugs!)

## Graph/Game

snake game: <https://github.com/rcore-os/rcore-user/blob/master/app/src/snake.c>

### Step1: compile usr app

We can use musl-gcc compile it in x86_64 mode

### Step2: change zcore configuration to run snake app first

Instead of modifying the kernel source code, modify the `cmdline` parameter in `zCore/rboot.conf` (or pass it via `CMDLINE` variable) to specify the initial process using the `ROOTPROC` option:

**Option A: Edit `zCore/rboot.conf`:**
Add `:ROOTPROC=/bin/snake` at the end of the `cmdline` setting:
```ini
cmdline=LOG=error:TERM=xterm-256color:console.shell=true:virtcon.disable=true:ROOTPROC=/bin/snake
```

**Option B: Pass via `CMDLINE` make variable:**
Pass the full command-line arguments directly to the make command:
```sh
make qemu GRAPHIC=on CMDLINE="LOG=error:TERM=xterm-256color:console.shell=true:virtcon.disable=true:ROOTPROC=/bin/snake"
```

### Step3: prepare root fs image, run zcore in linux-bare-metal mode

exec:

```sh
# Prepare rootfs
make rootfs

# Copy snake ELF file to the rootfs/x86_64/bin directory
cp /path/to/compiled/snake rootfs/x86_64/bin/

# Build rootfs image and run zcore with graphics enabled
make qemu GRAPHIC=on CMDLINE="LOG=error:TERM=xterm-256color:console.shell=true:virtcon.disable=true:ROOTPROC=/bin/snake"
```

Then you can play the game.

Operation

- Keyboard
  - `W`/`A`/`S`/`D`: Move
  - `R`: Restart
  - `ESC`: End
- Mouse
  - `Left`: Speed up
  - `Right`: Slow down
  - `Middle`: Pause/Resume

## Memory Hardening & Safety

To prevent kernel Out-of-Memory (OOM) panics caused by large memory requests and buffer allocations under constrained heap (BuddyAllocator) environments, the following safety strategies are implemented:

- **Chunked & Capped I/O Buffering**:
  - Temporary heap allocations for read system calls (`sys_read`, `sys_pread`, `sys_readv`, `sys_recvfrom`, and `sys_recvmsg`) are strictly capped at 1 MB. Large I/O requests are processed in chunks for seekable files.
  - Directory reads (`sys_getdents64`) are capped at 256 KB.
  - Symbolic link reads (`sys_readlinkat`) are capped at 4 KB.
- **Stack-based Buffer for Randomness**:
  - `sys_getrandom` uses a 1 KB stack buffer to completely avoid heap allocations.
- **Hardened ELF Loader (`execve`)**:
  - Instead of loading the entire ELF binary into a contiguous heap vector (via `read_as_vec`), the binary and its interpreter are read into a page-backed `VmObject`.
  - The `VmObject` is dynamically mapped on-demand to the kernel's virtual memory space (`KERNEL_ASPACE`) during the loading process, avoiding large contiguous physical memory allocations on the kernel heap.

## Doc

```
make doc
```

### RISC-V 64 porting info

- [porting riscv64 doc](./docs/porting-rv64.md)

## Components

### Overview

![](./docs/structure.svg)

[zircon]: https://fuchsia.googlesource.com/fuchsia/+/master/zircon/README.md
[kernel-objects]: https://github.com/PanQL/zircon/blob/master/docs/objects.md
[syscalls]: https://github.com/PanQL/zircon/blob/master/docs/syscalls.md

### Hardware Abstraction Layer

|                           | Bare Metal | Linux / macOS     |
| :------------------------ | ---------- | ----------------- |
| Virtual Memory Management | Page Table | Mmap              |
| Thread Management         | `executor` | `async-std::task` |
| Exception Handling        | Interrupt  | Signal            |

### Small Goal & Little Plans

- <https://github.com/rcore-os/zCore/wiki/Plans>
