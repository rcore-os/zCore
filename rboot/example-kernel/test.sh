#!/bin/bash
# End-to-end test for rboot: build bootloader + example kernel, run in QEMU
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RBOOT_DIR="$(dirname "$SCRIPT_DIR")"
ESP_DIR="$SCRIPT_DIR/esp"

echo "=== Building rboot ==="
cd "$RBOOT_DIR"
cargo build --release --target x86_64-unknown-uefi

echo "=== Building example kernel ==="
cd "$SCRIPT_DIR"
RUSTFLAGS="-C link-arg=--image-base=0xffffffff80000000 -C relocation-model=static" cargo build --release

echo "=== Preparing ESP ==="
rm -rf "$ESP_DIR"
mkdir -p "$ESP_DIR/EFI/Boot" "$ESP_DIR/EFI/zCore"
cp "$RBOOT_DIR/target/x86_64-unknown-uefi/release/rboot.efi" "$ESP_DIR/EFI/Boot/BootX64.efi"
cp "$SCRIPT_DIR/rboot.conf" "$ESP_DIR/EFI/Boot/rboot.conf"
cp "$SCRIPT_DIR/target/x86_64-unknown-none/release/example-kernel" "$ESP_DIR/EFI/zCore/example-kernel"

echo "=== Running QEMU ==="
OVMF="$RBOOT_DIR/OVMF.fd"
if [ ! -f "$OVMF" ]; then
    echo "ERROR: OVMF.fd not found at $OVMF"
    exit 1
fi

OUTPUT=$(timeout 15 qemu-system-x86_64 \
    -machine q35 \
    -cpu qemu64 \
    -m 256M \
    -smp 1 \
    -serial stdio \
    -drive format=raw,if=pflash,readonly=on,file="$OVMF" \
    -drive format=raw,file=fat:rw:"$ESP_DIR" \
    -nic none \
    -display none \
    -no-reboot \
    -device isa-debug-exit,iobase=0x501,iosize=2 \
    2>/dev/null || true)

echo "$OUTPUT"

if echo "$OUTPUT" | grep -q "rboot is working correctly"; then
    echo ""
    echo "=== TEST PASSED ==="
    exit 0
else
    echo ""
    echo "=== TEST FAILED ==="
    echo "Expected output containing 'rboot is working correctly'"
    exit 1
fi
