# zCore

[![Actions Status](https://github.com/rcore-os/zircon-rs/workflows/CI/badge.svg)](https://github.com/rcore-os/zircon-rs/actions)
[![Docs](https://img.shields.io/badge/docs-alpha-blue)](https://rcore-os.github.io/zircon-rs/zircon_object/)
[![Coverage Status](https://coveralls.io/repos/github/rcore-os/zircon-rs/badge.svg?branch=master)](https://coveralls.io/github/rcore-os/zircon-rs?branch=master)

Reimplement [Zircon][zircon] microkernel in safe Rust as a userspace program!

🚧 Working In Progress

## Getting started

```sh
git clone https://github.com/rcore-os/zircon-rs
git lfs pull
cd zircon-rs
```

Prepare Alpine Linux rootfs:

```sh
make rootfs
```

Run native Linux program (Busybox):

```sh
cargo run --release -p linux-loader /bin/busybox [args]
```

Run native Zircon program (userboot):

```sh
cargo run --release -p zircon-loader prebuilt/userboot.so prebuilt/libzircon.so prebuilt/legacy-image-x64.zbi
```

To debug, set `RUST_LOG` environment variable to one of `error`, `warn`, `info`, `debug`, `trace`.

## Components

### Overview

![](./docs/structure.svg)

[zircon]: https://fuchsia.googlesource.com/fuchsia/+/master/zircon/README.md
[kernel-objects]: https://github.com/PanQL/zircon/blob/master/docs/objects.md
[syscalls]: https://github.com/PanQL/zircon/blob/master/docs/syscalls.md

### Hardware Abstraction Layer

|                           | Bare Metal     | Linux / macOS |
| :------------------------ | -------------- | ------------- |
| Virtual Memory Management | Page Table     | Mmap          |
| Thread Management         | `rcore-thread` | `std::thread` |
| Exception Handling        | Interrupt      | Signal        |

