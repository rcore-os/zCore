# zCore

[![CI](https://github.com/rcore-os/zCore/workflows/CI/badge.svg?branch=master)](https://github.com/rcore-os/zCore/actions)
[![Docs](https://img.shields.io/badge/docs-alpha-blue)](https://rcore-os.github.io/zCore/zircon_object/)
[![Coverage Status](https://coveralls.io/repos/github/rcore-os/zCore/badge.svg?branch=master)](https://coveralls.io/github/rcore-os/zCore?branch=master)

Reimplement [Zircon][zircon] microkernel in safe Rust as a userspace program!

## Dev Status

🚧 Working In Progress

- 2020.04.16: Zircon console is working on zCore! 🎉

## Getting started

Environments：

* [Rust toolchain](http://rustup.rs)
* [QEMU](https://www.qemu.org)
* [Git LFS](https://git-lfs.github.com)

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

Prepare Alpine Linux rootfs:

```sh
make rootfs
```

Run native Linux program (Busybox):

```sh
cargo run --release -p linux-loader -- /bin/busybox [args]
```

Run native Zircon program (shell):

```sh
cargo run --release -p zircon-loader -- prebuilt/zircon/x64
```

Run Linux shell on bare-metal (zCore):

```sh
make image
cd zCore && make run mode=release linux=1 [graphic=on] [accel=1]
```

Run Zircon on bare-metal (zCore):

```sh
cd zCore && make run mode=release [graphic=on] [accel=1]
```

Build and run your own Zircon user programs:

```sh
# See template in zircon-user
cd zircon-user && make zbi mode=release

# Run your programs in zCore
cd zCore && make run mode=release user=1
```

To debug, set `RUST_LOG` environment variable to one of `error`, `warn`, `info`, `debug`, `trace`.

## Testing

Run Zircon official core-tests:

```sh
cd zCore && make test mode=release [accel=1] test_filter='Channel.*'
```

Run all (non-panicked) core-tests for CI:

```sh
pip3 install pexpect
cd scripts && python3 core-tests.py
# Check `zircon/test-result.txt` for results.
```


Run Linux musl libc-tests for CI:

```sh
make rootfs && make libc-test
cd scripts && python3 libc-tests.py
# Check `linux/test-result.txt` for results.
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

### Some plans
- https://github.com/rcore-os/zCore/wiki/Plans
