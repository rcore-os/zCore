# zCore

[![Actions Status](https://github.com/rcore-os/zircon-rs/workflows/CI/badge.svg)](https://github.com/rcore-os/zircon-rs/actions)
[![Docs](https://img.shields.io/badge/docs-alpha-blue)](https://rcore-os.github.io/zircon-rs/zircon_object/)
[![Coverage Status](https://coveralls.io/repos/github/rcore-os/zircon-rs/badge.svg?branch=master)](https://coveralls.io/github/rcore-os/zircon-rs?branch=master)

Reimplement [Zircon][zircon] microkernel in safe Rust as a userspace program!

🚧 Working In Progress

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

