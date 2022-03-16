# zCore Get Started

[![CI](https://github.com/rcore-os/zCore/workflows/CI/badge.svg?branch=master)](https://github.com/rcore-os/zCore/actions)
[![Docs](https://img.shields.io/badge/docs-alpha-blue)](https://rcore-os.github.io/zCore/)
[![Coverage Status](https://coveralls.io/repos/github/rcore-os/zCore/badge.svg?branch=master)](https://coveralls.io/github/rcore-os/zCore?branch=master)

Reimplement [Zircon][zircon] microkernel in safe Rust as a userspace program!

## Quick start for RISCV64

```sh
make riscv-image
cd zCore
make run ARCH=riscv64 LINUX=1
```

## Getting started

Environments：

* [Rust toolchain](http://rustup.rs)
* [QEMU](https://www.qemu.org)
* [Git LFS](https://git-lfs.github.com)

### Developing environment info

- current rustc -- rustc 1.60.0-nightly (5e57faa78 2022-01-19)
- current rust-toolchain -- nightly-2022-01-20
- current qemu -- 5.2.0 -> 6.2.0

Clone repo and pull prebuilt fuchsia images:

```sh
git clone https://github.com/rcore-os/zCore --recursive
cd zCore
git lfs install
git lfs pull
```

For users in China, there's a mirror you can try:

```sh
git clone https://github.com.cnpmjs.org/rcore-os/zCore --recursive
```
### Run zcore in libos mode
#### Run zcore in linux-libos mode
* step 1: Prepare Alpine Linux rootfs:

  ```sh
  make rootfs
  ```

* step 2: Compile & Run native Linux program (Busybox) in libos mode:

  ```sh
  cargo run --release --features "linux libos" -- /bin/busybox [args]
  ```

  You can also add the feature `graphic` to show the graphical output (with [sdl2](https://www.libsdl.org) installed).

  To debug, set the `LOG` environment variable to one of `error`, `warn`, `info`, `debug`, `trace`.

#### Run native Zircon program (shell) in zircon-libos mode:

* step 1: Compile and Run Zircon shell

  ```sh
  cargo run --release --features "zircon libos" -- prebuilt/zircon/x64/bringup.zbi
  ```

  The `graphic` and `LOG` options are the same as Linux.

### Run zcore in bare-metal mode
#### Run Linux shell in  linux-bare-metal mode:

* step 1: Prepare Alpine Linux rootfs:

  ```sh
  make rootfs
  ```

* step 2: Create Linux rootfs image:

  Note: Before below step, you can add some special apps in zCore/rootfs

  ```sh
  make image
  ```

* step 3: Build and run zcore in  linux-bare-metal mode:

  ```sh
  cd zCore && make run MODE=release LINUX=1 [LOG=warn] [GRAPHIC=on] [ACCEL=1]
  ```

#### Run Zircon shell in zircon-bare-metal mode:

* step 1: Build and run zcore in  zircon-bare-metal mode:

  ```sh
  cd zCore && make run MODE=release [LOG=warn] [GRAPHIC=on] [ACCEL=1]
  ```

* step 2: Build and run your own Zircon user programs:

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
pip3 install -r tests/requirements.txt
cd tests && python3 zircon_core_test.py --libos -f
# Check `zircon_core_test_x86_64_libos.log` for results.
```

Run all (non-panicked) core-tests for CI:

```sh
pip3 install -r tests/requirements.txt
cd tests && python3 zircon_core_test.py --libos
# Check `zircon_core_test_x86_64_libos.log` for results.
```

#### Linux related

Run Linux musl libc-tests for CI:

```sh
make rootfs && make libc-test
cd tests && python3 linux_libc_test.py --libos
# Check `linux_libc_test_x86_64_libos.log` for results.
```

### Bare-metal Mode Testing
#### Zircon related

Run Zircon official core-tests on bare-metal:

```sh
cd tests && python3 zircon_core_test.py -f
# Check `zircon_core_test_x86_64_bare.log` for results.
```

Run all (non-panicked) core-tests for CI:

```sh
cd tests && python3 zircon_core_test.py
# Check `zircon_core_test_x86_64_bare.log` for results.
```

#### x86-64 Linux related

Run Linux musl libc-tests for CI:
```sh
##  Prepare rootfs with libc-test apps
make baremetal-test-img
## Testing
cd tests && python3 linux_libc_test.py --arch x86_64
# Check `linux_libc_test_x86_64_bare.log` for results.
```

You can use [`tests/linux_libc_test.py -a x86_64 -t <testcase>`](./tests/linux_libc_test.py)  to test specified apps.

#### riscv-64 Linux related

Run Linux musl libc-tests for CI:
```sh
##  Prepare rootfs with libc-test & oscomp apps
make riscv-image
## Build zCore kernel & Testing
cd tests && python3 linux_libc_test.py --arch riscv64
# Check `linux_libc_test_riscv64_bare.log` for results.
```

You can use[ `tests/linux_libc_test.py -a riscv64 -t <testcase>`](./tests/linux_libc_test.py) to test
specified apps.

## Graph/Game

snake game: https://github.com/rcore-os/rcore-user/blob/master/app/src/snake.c

### Step1: compile usr app
We can use musl-gcc compile it in x86_64 mode

### Step2: change zcore for run snake app first.
change zCore/zCore/main.rs L176
vec!["/bin/busybox".into(), "sh".into()]
TO
vec!["/bin/snake".into(), "sh".into()]

### Step3: prepare root fs image, run zcore in linux-bare-metal mode
exec:

```sh
cd zCore #zCore ROOT DIR
make rootfs
cp ../rcore-user/app/snake rootfs/bin #copy snake ELF file to rootfs/bin
make image # build rootfs image
cd zCore #zCore kernel dir
make run MODE=release LINUX=1 GRAPHIC=on
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

## Doc
```
make doc
```
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
- https://github.com/rcore-os/zCore/wiki/Plans
