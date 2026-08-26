#!/usr/bin/env bash
#
# X11 kernel-interaction bench for Eclipse OS (zCore).
#
# Boots zCore under QEMU (UEFI/OVMF) running a tiny static init that exercises
# exactly the kernel surface an X server uses — framebuffer (open + screeninfo
# + mmap + draw), evdev (open + EVIOCGNAME/EVIOCGBIT), TIOCGWINSZ on a pipe, and
# the AF_UNIX connect/accept handshake (client writes before the server
# accepts). Prints the `XTEST:` lines so a regression is obvious.
#
# Usage:  tools/x11-bench/run.sh
#
# Requires: qemu-system-x86_64, the x86_64-linux-musl-cross toolchain under
# target/x86_64/ (cargo rootfs downloads it), and rboot/OVMF.fd.
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT=$PWD

MUSL=target/x86_64/x86_64-linux-musl-cross/bin/x86_64-linux-musl-gcc
ESP=target/x86_64/release/esp
OVMF=rboot/OVMF.fd

echo "== building static test init =="
"$MUSL" -static -no-pie -O2 -o /tmp/xtest_bin tools/x11-bench/xtest.c

echo "== staging minimal rootfs =="
mkdir -p rootfs/x86_64/bin rootfs/x86_64/tmp rootfs/x86_64/dev
cp /tmp/xtest_bin rootfs/x86_64/bin/xtest

echo "== packing SFS initramfs (zCore/x86_64.img) =="
cargo test -p xtask dbg_repack_initramfs -- --nocapture >/dev/null 2>&1

echo "== building kernel + bootloader, assembling ESP (ROOTPROC=/bin/xtest) =="
make -C zCore build ARCH=x86_64 MODE=release LINUX=1 GRAPHIC=on \
     LOG=info CMDLINE="LOG=info:ROOTPROC=/bin/xtest" >/dev/null

echo "== booting QEMU (headless, TCG) =="
OUT=/tmp/x11-bench.out
timeout 150 qemu-system-x86_64 \
  -machine q35 -cpu Haswell,+smap,-check,-fsgsbase -m 2G \
  -serial stdio \
  -drive format=raw,if=pflash,readonly=on,file="$OVMF" \
  -drive format=raw,file=fat:rw:"$ESP" \
  -vga std -display none -nic none -no-reboot \
  < /dev/null > "$OUT" 2>&1 || true

echo "== results =="
grep -a "XTEST" "$OUT" | tr -d '\r' || { echo "NO XTEST OUTPUT — see $OUT"; exit 1; }
# The init returns 0 and prints "N/N passed" only when every check passed.
if grep -aE "XTEST: ([0-9]+)/\1 passed" "$OUT" >/dev/null && grep -aq "XTEST: done" "$OUT"; then
    echo "BENCH OK"
else
    echo "BENCH FAILED"; exit 1
fi
