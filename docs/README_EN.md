# zCore (Eclipse OS)

An operating system kernel based on Zircon that provides Linux compatibility.

- [Spanish README](../README.md)
- [Legacy upstream README](../README-arch.md)

  > You may want to check the legacy README for setting up docker, running graphical applications, and other upstream details. Note that many of those scripts are deprecated.

## Project Overview

zCore is a reimplementation of the `Zircon` microkernel in safe Rust as a userspace program.

- zCore design architecture.
- Support for Zircon and Linux in bare-metal mode.
- Support for Zircon and Linux in libos mode.
- For more guides on graphical applications and other details, see the [original architecture documentation](../README-arch.md).

## Launch the Kernel

```bash
make qemu ARCH=x86_64
```

This command launches zCore using QEMU for the specified architecture.

The default file system includes the `busybox` application and the `musl-libc` library. These are compiled automatically using the corresponding cross-compilation toolchain.

## Initial Process Configuration (ROOTPROC)

To change the initial process (init) that zCore runs at boot, edit the configuration file `zCore/rboot.conf`.

In that file, locate the `cmdline` line and add the `ROOTPROC` parameter. Parameters on the command line are separated by the `:` character.

**Example to run a busybox shell (default):**
```ini
cmdline=LOG=warn:ROOTPROC=/bin/busybox?sh
```

**Example to run a specific binary with arguments:**
```ini
cmdline=LOG=warn:ROOTPROC=/path/to/init?--option?value
```

**Format:**
- `ROOTPROC=/path/to/binary`: Specifies the path of the executable in the file system.
- `?`: Used to separate the command from its arguments and the arguments from each other.

## Table of Contents

- [Launch the Kernel](#launch-the-kernel)
- [Initial Process Configuration (ROOTPROC)](#initial-process-configuration-rootproc)
- [Building the Project](#building-the-project)
  - [Build Commands](#build-commands)
  - [Command Reference](#command-reference)
- [Platform Support](#platform-support)
  - [x86_64 (QEMU and Real Hardware)](#x86_64-qemu-and-real-hardware)
  - [Qemu/virt (RISC-V)](#qemuvirt-risc-v)
  - [Allwinner D1/Nezha](#allwinner-d1nezha)
  - [StarFive VisionFive](#starfive-visionfive)
  - [CVITEK CR1825](#cvitek-cr1825)

## Building the Project

The build uses the [xtask pattern](https://github.com/matklad/cargo-xtask). Common operations are wrapped as `cargo` commands.

In addition, a [Makefile](../Makefile) is provided for compatibility with some legacy scripts.

The currently tested development environments include Ubuntu 20.04, Ubuntu 22.04, and Debian 11.

### Build Commands

The basic format of the commands is `cargo <command> [--args [value]]`. This is actually shorthand for `cargo run --package xtask --release -- <command> [--args [value]]`. The command is passed to the xtask application for parsing and execution.

Many commands depend on others to prepare the environment. The dependency diagram is as follows:

```text
┌────────────┐ ┌─────────────┐ ┌─────────────┐
| update-all | | check-style | | zircon-init |
└────────────┘ └─────────────┘ └─────────────┘
┌─────┐ ┌──────┐  ┌─────┐  ┌─────────────┐ ┌─────────────────┐
| asm | | qemu |─→| bin |  | linux-libos | | libos-libc-test |
└─────┘ └──────┘  └─────┘  └─────────────┘ └─────────────────┘
                     |            └───┐┌─────┘   ┌───────────┐
                     ↓                ↓↓      ┌──| libc-test |
                 ┌───────┐        ┌────────┐←─┘  └───────────┘
                 | image |───────→| rootfs |←─┐ ┌────────────┐
                 └───────┘        └────────┘  └─| other-test |
                 ┌────────┐           ↑         └────────────┘
                 | opencv |────→┌───────────┐
                 └────────┘  ┌─→| musl-libc |
                 ┌────────┐  |  └───────────┘
                 | ffmpeg |──┘
                 └────────┘
-------------------------------------------------------------------
Legend: A → B (A depends on B; running A will automatically run B first)
```

### Command Reference

#### **update-all**
Updates the toolchain, dependencies, and git submodules.
```bash
cargo update-all
```

#### **check-style**
Static check. Verifies that the code compiles with various options.
```bash
cargo check-style
```

#### **zircon-init**
Downloads the binaries needed for Zircon mode.
```bash
cargo zircon-init
```

#### **qemu**
Launches zCore in QEMU. Requires QEMU to be installed.
```bash
cargo qemu --arch x86_64 --smp 4
```

Connecting QEMU to GDB:
```bash
cargo qemu --arch x86_64 --smp 4 --gdb 1234
```

#### **rootfs**
Rebuilds the Linux rootfs.
```bash
cargo rootfs --arch x86_64
```

#### **image**
Builds the Linux rootfs image file from the corresponding directory.
```bash
cargo image --arch x86_64
```

## Platform Support

### x86_64 (QEMU and Real Hardware)

Full support for the x86_64 architecture on emulators (QEMU) and on real hardware, with significant compatibility improvements:

- **AHCI/SATA Driver**: Improved support with robust initialization that includes the BIOS/OS handoff protocol, PHY physical link stabilization (SATA DET), and flexible device signature verification (`PORT_SIG`). PCI Bus Mastering is also enabled to prevent Master Abort failures on real hardware.
- **NVMe Driver**: Support for NVMe storage controllers with DMA cache consistency using `clflush` instructions.
- **Automatic Detection and Partitioning**: Dynamic detection of MBR and GPT partitioning schemes at system boot. Partitions (such as `/dev/sda1` or `/dev/nvme0n1p1`) are automatically registered in `devfs` and exposed as independent devices.
- **Input and Keyboard**: PS/2 keyboard support with full mapping of the Spanish keyboard layout, allowing the correct use of special characters and accents (`ñ`, `Ñ`, `@`, `#`, `[`, `]`, `{`, `}`, `|`, `\`, `~`, `€`) through modifiers (AltGr and Shift).
- **System Installer (`install-eclipse`)**: An installation tool optimized for deploying the system to physical and virtual disks, with precise disk-size detection combining `sysfs` queries and the `BLKGETSIZE64` call. It writes and modifies directly on the partition devices (e.g. `/dev/sda1` and `/dev/sda2`) to guarantee block-cache consistency and the correct persistence of key configuration files (`/etc/fstab` and `rboot.conf`).
- **File Systems**: The root file system of Eclipse OS is **btrfs**, with its own in-kernel read/write driver (crate `vendor/btrfs-rs`) and image generation integrated into the build (without depending on `btrfs-progs`); the file system automatically expands to the partition size on the first mount. **ext2/ext3/ext4** support is maintained (old installations and external disks), as is **vfat/FAT32** (EFI partition). The generated btrfs images are mountable by Linux and pass `btrfs check`.
- **Memory Stability Under Pressure (OOM)**: Mitigation of kernel panics caused by heap exhaustion (BuddyAllocator) through strict temporary allocation limits (1 MB) and chunked processing in I/O syscalls (`sys_read`, `sys_pread`, etc.), and a robust ELF loading strategy (`sys_execve`) using on-demand dynamic mappings of paged `VmObject`s in the kernel virtual region (`KERNEL_ASPACE`) without allocating contiguous physical memory.
- **Graphics Stack (DRM/KMS)**: Implementation of the Linux DRM/KMS UAPI that allows running standard graphics software — `Xorg` (`startx`) via the virtual console nodes and the VT/KD `ioctl`s, and Wayland compositors (`wlroots`/`labwc`, with `WLR_RENDERER=pixman` by default when no GPU is present; forcing `WLR_RENDERER=gles2` falls back to Mesa `llvmpipe` and is extremely slow). Includes PRIME support (dma-buf export/import). See [README-drm.md](README-drm.md) and [README-xorg.md](README-xorg.md).
- **Security (`hunter`)**: An in-kernel security subsystem that combines an LSM-style policy-enforcement layer with a behavioural intrusion-detection system (IDS), recording every decision in a forensic log readable from `/proc/hunter`. See [hunter-security.md](hunter-security.md).
- **Status**: The system boots successfully on real hardware, initializes the storage controllers, mounts the file system natively, and starts the interactive console (`busybox`).

### Qemu/virt (RISC-V)

Launch directly using cargo commands, see [Launch the Kernel](#launch-the-kernel).

### Allwinner D1/Nezha

Use the following command to build the system image:
```bash
cargo bin -m nezha -o z.bin
```
Then use [rustsbi-d1](https://github.com/rustsbi/rustsbi-d1) to deploy the image to Flash or DRAM.

### StarFive VisionFive

Use the following command to build the image:
```bash
cargo bin -m visionfive -o z.bin
```

### CVITEK CR1825

Use the following command to build the image:
```bash
cargo bin -m cr1825 -o z.bin
```

## Package Management (APK Tools)

zCore (Eclipse OS) uses `apk-tools` as its package manager. To build it and prepare the environment:

To install the Alpine trusted keys:
```bash
apk add -X https://dl-cdn.alpinelinux.org/alpine/v3.24/main -u alpine-keys
```

## Documentation

### Graphics and desktop environment
- [DRM / KMS — Linux UAPI conformance](README-drm.md)
- [Running an X server (`startx`)](README-xorg.md)

### Security
- [hunter — in-kernel security subsystem](hunter-security.md)
- [hunter — hardening (red-team) report](hunter-hardening.md)

### RISC-V platforms
- [StarFive VisionFive](README-visionfive.md)
- [Allwinner D1/Nezha](README-D1.md)
- [Sophgo/CVITEK C910](README-C910.md)
- [StarFive JH7110 (FU740)](README-fu740.md)
- [RISC-V 64 porting notes](porting-rv64.md)

## Others

- [Spanish README](../README.md)
- [Developer notes](for-developers.md)
- [Original architecture documentation (upstream zCore)](../README-arch.md)
- [Build system changelog](../xtask/CHANGELOG.md)
</content>
</invoke>
