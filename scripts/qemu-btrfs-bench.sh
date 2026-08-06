#!/usr/bin/env bash
# btrfs-on-AHCI comparison: run eclipse-bench's DISK section against a real
# btrfs filesystem on a dedicated SATA/AHCI disk, on Eclipse and on Linux, and
# print the two side by side.
#
# Why a separate disk. The default DISK row runs in the shell's cwd, which on
# both harnesses is a RAM-backed root — it measures the page cache, not a
# filesystem on a block device. This attaches an identical raw btrfs image to
# both kernels through the same ich9-ahci controller, mounts it, and benchmarks
# THERE, so the number reflects the btrfs write/read/metadata paths over the
# AHCI driver and nothing else.
#
# Fairness. Both kernels get a byte-identical, freshly-`mkfs`'d image each run
# (a copy — QEMU writes in place). The btrfs is formatted `-O ^free-space-tree`
# with crc32c and 16 KiB nodes, which is exactly the layout Eclipse's in-tree
# `btrfs` crate emits and reads (see vendor/btrfs-rs/src/mkfs.rs); Linux mounts
# anything mkfs.btrfs produces. The Linux side loads the modular ahci + btrfs
# stack from the host's own /lib/modules before mounting.
#
# Usage: scripts/qemu-btrfs-bench.sh [-s SIZE_MB] [-w WORK_MB] [-o OUTDIR]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SIZE_MB=1024      # disk image size
WORK_MB=128       # eclipse-bench DISK working set
OUTDIR=""
TIMEOUT=1800

while getopts "s:w:o:t:" opt; do
    case "$opt" in
        s) SIZE_MB="$OPTARG" ;;
        w) WORK_MB="$OPTARG" ;;
        o) OUTDIR="$OPTARG" ;;
        t) TIMEOUT="$OPTARG" ;;
        *) echo "usage: $0 [-s SIZE_MB] [-w WORK_MB] [-o OUTDIR] [-t TIMEOUT]" >&2; exit 2 ;;
    esac
done

command -v mkfs.btrfs >/dev/null || { echo "$0: mkfs.btrfs not found (apt install btrfs-progs)" >&2; exit 1; }

WORK="$(mktemp -d)"
[ -n "$OUTDIR" ] || OUTDIR="$WORK"
mkdir -p "$OUTDIR"
cleanup() { rm -rf "$WORK" 2>/dev/null; }
trap cleanup EXIT INT TERM

# One pristine master image, formatted the way Eclipse's crate expects.
MASTER="$WORK/btrfs-master.img"
truncate -s "${SIZE_MB}M" "$MASTER"
mkfs.btrfs -f -q \
    -O ^free-space-tree,^extref,^skinny-metadata,^no-holes \
    --csum crc32c --nodesize 16384 --sectorsize 4096 -L ECLBENCH "$MASTER" >/dev/null
echo "$0: formatted ${SIZE_MB} MiB btrfs (^free-space-tree, crc32c, 16K nodes)" >&2

# Each kernel gets its own fresh copy — QEMU writes into the image in place, so
# reusing one would let the first run's writes skew the second.
ECL_IMG="$WORK/btrfs-eclipse.img"
LIN_IMG="$WORK/btrfs-linux.img"
cp "$MASTER" "$ECL_IMG"
cp "$MASTER" "$LIN_IMG"

BENCH_CMD="eclipse-bench --only disk /mnt/bt $WORK_MB"
MOUNT_CMD="mkdir -p /mnt/bt; mount -t btrfs /dev/sda /mnt/bt"

echo "$0: === Eclipse (btrfs on AHCI) ===" >&2
"$ROOT/scripts/qemu-bench.sh" -t "$TIMEOUT" -d "$ECL_IMG" -c "VDSOFORCE=1:ROOTKEEP=1" \
    -o "$OUTDIR/btrfs-eclipse.txt" \
    "$MOUNT_CMD && echo MOUNTED_ECLIPSE || echo MOUNT_FAILED" \
    "$BENCH_CMD" \
    "sync; umount /mnt/bt" >/dev/null 2>&1 || true

echo "$0: === Linux (btrfs on AHCI) ===" >&2
"$ROOT/scripts/qemu-linux-bench.sh" -t "$TIMEOUT" -d "$LIN_IMG" \
    -o "$OUTDIR/btrfs-linux.txt" \
    "$MOUNT_CMD && echo MOUNTED_LINUX || echo MOUNT_FAILED" \
    "$BENCH_CMD" \
    "sync; umount /mnt/bt" >/dev/null 2>&1 || true

# ── Side-by-side ─────────────────────────────────────────────────────────────
echo
echo "btrfs-on-AHCI: Eclipse vs Linux 6.8 (same QEMU, ${WORK_MB} MiB working set)"
echo "-------------------------------------------------------------------------"
for f in eclipse linux; do
    grep -q "MOUNTED_${f^^}" "$OUTDIR/btrfs-$f.txt" 2>/dev/null \
        || echo "WARNING: $f run did not report a successful mount — check $OUTDIR/btrfs-$f.txt" >&2
done
python3 - "$OUTDIR/btrfs-eclipse.txt" "$OUTDIR/btrfs-linux.txt" <<'PY'
import re, sys
def rows(path):
    out = {}
    try:
        for line in open(path, errors="replace"):
            m = re.match(r"\s+\[kernel\]\s+(.+?)\s{2,}([\d.]+|n/a)\s+(\S+)", line)
            if m:
                out.setdefault(m.group(1).strip(), (m.group(2), m.group(3)))
    except FileNotFoundError:
        pass
    return out
e, l = rows(sys.argv[1]), rows(sys.argv[2])
HIGHER = ("MB/s", "IOPS", "files/s", "stats/s", "unlinks/s")
print(f"{'metric':<28} {'Eclipse':>14} {'Linux':>14}  verdict")
for k in e:
    if k not in l:
        continue
    ev, eu = e[k]; lv, lu = l[k]
    verdict = ""
    try:
        a, b = float(ev), float(lv)
        if min(a, b) > 0:
            hb = any(u in eu for u in HIGHER)
            r = a / b if hb else b / a
            verdict = "tie" if 0.9 < r < 1.11 else f"{'Eclipse' if r > 1 else 'Linux'} {max(r,1/r):.1f}x"
    except ValueError:
        pass
    print(f"{k:<28} {ev:>10} {eu:<3} {lv:>10} {lu:<3}  {verdict}")
PY
echo
echo "logs: $OUTDIR/btrfs-{eclipse,linux}.txt"
