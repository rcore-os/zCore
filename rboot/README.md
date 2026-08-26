# rBoot

The x86_64 UEFI bootloader for rCore / zCore OS.

## Build

```sh
cargo build --release --target x86_64-unknown-uefi
```

The output EFI binary is at `target/x86_64-unknown-uefi/release/rboot.efi`.

## Example

See [`example-kernel/`](example-kernel/) for a minimal bare-metal kernel that boots via rboot and prints to serial.

Run `example-kernel/test.sh` to build and test in QEMU.

## Configuration

Edit `rboot.conf` to configure the bootloader. See [`example-kernel/rboot.conf`](example-kernel/rboot.conf) for a working example. Available options:

- `kernel_path` - path to the kernel ELF binary
- `kernel_stack_address` - virtual address for the kernel stack
- `kernel_stack_size` - kernel stack size in 4KiB pages
- `physical_memory_offset` - virtual address offset for physical memory mapping
- `resolution` - graphic output resolution (e.g. `1024x768`)
- `initramfs` - path to the initial ramdisk image
- `cmdline` - kernel command line
