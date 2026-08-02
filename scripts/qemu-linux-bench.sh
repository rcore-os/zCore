#!/usr/bin/env bash
#
# qemu-linux-bench.sh — the Linux control for scripts/qemu-bench.sh.
#
# Boots a stock Linux kernel under *the same* QEMU machine (same -cpu, -smp,
# -m, same TCG emulation, no KVM) with a minimal busybox initramfs, and runs the
# same commands on the same kind of serial console.
#
# This is the only comparison that settles anything. Running eclipse-bench on
# the host and on an emulated Eclipse compares an emulated CPU against a real
# one, which tells you nothing about the two kernels. Running both kernels under
# identical emulation cancels the hardware out.
#
# Usage:
#   scripts/qemu-linux-bench.sh [-o OUTFILE] [-t TIMEOUT] [-s SMP] [-m MEM]
#                               [-k KERNEL] CMD [CMD...]
#
#   -k KERNEL   bzImage to boot (default: /boot/vmlinuz, or $LINUX_KERNEL)
#
# The initramfs is built from the busybox that `cargo rootfs` already compiled
# (ignored/target/x86_64/busybox/busybox) plus the eclipse-bench binary from
# rootfs/x86_64/bin, so guest userland is byte-identical to Eclipse's.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUSYBOX="$ROOT/ignored/target/x86_64/busybox/busybox"
BENCH="$ROOT/rootfs/x86_64/bin/eclipse-bench"

OUTFILE=""
TIMEOUT=1800
SMP=4
MEM=4G
KERNEL="${LINUX_KERNEL:-/boot/vmlinuz}"

while getopts "o:t:s:m:k:" opt; do
    case "$opt" in
        o) OUTFILE="$OPTARG" ;;
        t) TIMEOUT="$OPTARG" ;;
        s) SMP="$OPTARG" ;;
        m) MEM="$OPTARG" ;;
        k) KERNEL="$OPTARG" ;;
        *) echo "usage: $0 [-o OUT] [-t TIMEOUT] [-s SMP] [-m MEM] [-k KERNEL] CMD..." >&2; exit 2 ;;
    esac
done
shift $((OPTIND - 1))

[ $# -gt 0 ] || { echo "$0: no commands given" >&2; exit 2; }
[ -f "$KERNEL" ] || { echo "$0: no kernel at $KERNEL (set -k or \$LINUX_KERNEL)" >&2; exit 1; }
[ -x "$BUSYBOX" ] || { echo "$0: no busybox at $BUSYBOX — run: cargo rootfs --arch x86_64" >&2; exit 1; }
[ -x "$BENCH" ] || { echo "$0: no eclipse-bench at $BENCH — run: cargo rootfs --arch x86_64" >&2; exit 1; }

WORK="$(mktemp -d)"
[ -n "$OUTFILE" ] || OUTFILE="$WORK/console.log"
FIFO="$WORK/stdin.fifo"
mkfifo "$FIFO"

QEMU_PID=""
cleanup() {
    [ -n "$QEMU_PID" ] && kill "$QEMU_PID" 2>/dev/null
    exec 3>&- 2>/dev/null
    rm -rf "$WORK" 2>/dev/null
}
trap cleanup EXIT INT TERM

# ── initramfs ────────────────────────────────────────────────────────────────
IRD="$WORK/initramfs"
mkdir -p "$IRD"/{bin,dev,proc,sys,tmp,root,run}
cp "$BUSYBOX" "$IRD/bin/busybox"
cp "$BENCH" "$IRD/bin/eclipse-bench"
( cd "$IRD/bin" && for a in sh ls cat mount echo sleep nproc uname poweroff dmesg grep; do
      ln -sf busybox "$a"
  done )

cat > "$IRD/init" <<'EOF'
#!/bin/sh
/bin/busybox mount -t proc  proc  /proc
/bin/busybox mount -t sysfs sysfs /sys
/bin/busybox mount -t devtmpfs dev /dev 2>/dev/null
/bin/busybox mount -t tmpfs tmpfs /tmp
export PATH=/bin
# Match the prompt the Eclipse harness waits for, so both logs parse the same.
exec /bin/busybox sh -i
EOF
chmod +x "$IRD/init"

( cd "$IRD" && find . | cpio -o -H newc --quiet | gzip -9 ) > "$WORK/initramfs.cpio.gz"

# ── boot ─────────────────────────────────────────────────────────────────────
# Same machine, CPU, core count and memory as scripts/qemu-bench.sh, and equally
# without KVM. `quiet loglevel=0` keeps kernel chatter out of the captured log.
qemu-system-x86_64 \
    -smp "$SMP" \
    -machine q35 \
    -cpu Haswell,+smap,-check,-fsgsbase \
    -m "$MEM" \
    -serial mon:stdio \
    -kernel "$KERNEL" \
    -initrd "$WORK/initramfs.cpio.gz" \
    -append "console=ttyS0 quiet loglevel=0 rdinit=/init" \
    -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0 -device usb-tablet,bus=xhci.0 \
    -nic none \
    -display none \
    -no-reboot \
    < "$FIFO" > "$OUTFILE" 2>&1 &
QEMU_PID=$!

exec 3>"$FIFO"

deadline=$(( $(date +%s) + TIMEOUT ))

wait_for() {
    local pattern="$1" count="$2"
    while :; do
        if [ "$(grep -c -- "$pattern" "$OUTFILE" 2>/dev/null)" -ge "$count" ]; then
            return 0
        fi
        if ! kill -0 "$QEMU_PID" 2>/dev/null; then
            echo "$0: QEMU exited before '$pattern' appeared" >&2
            return 1
        fi
        if [ "$(date +%s)" -ge "$deadline" ]; then
            # Re-check before giving up: the poll below sleeps a second, so a
            # marker that lands inside that window would otherwise be reported
            # as a timeout on a run that actually completed.
            if [ "$(grep -c -- "$pattern" "$OUTFILE" 2>/dev/null)" -ge "$count" ]; then
                return 0
            fi
            echo "$0: timed out waiting for '$pattern'" >&2
            return 1
        fi
        sleep 1
    done
}

wait_for '#' 1 || exit 1

# The marker is split by a quote in the command we *type* ("__ECLIPSE""_BENCH..")
# so the line the shell echoes back can never match it — only the shell's own
# `echo` produces the joined string. Counting occurrences instead would be
# fragile: whether the console echoes typed input at all varies with how the
# guest set the terminal up, and getting that wrong reports a completed run as
# a timeout.
n=0
for cmd in "$@"; do
    n=$((n + 1))
    marker="__ECLIPSE_BENCH_DONE_${n}__"
    printf '%s; echo "__ECLIPSE""_BENCH_DONE_%d__"\n' "$cmd" "$n" >&3
    wait_for "$marker" 1 || exit 1
done

printf 'poweroff -f\n' >&3
sleep 3
kill "$QEMU_PID" 2>/dev/null
wait "$QEMU_PID" 2>/dev/null

echo "console log: $OUTFILE" >&2
