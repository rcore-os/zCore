#!/usr/bin/env bash
#
# qemu-bench.sh — boot Eclipse under QEMU, run commands on the serial console,
# capture the output, shut the machine down.
#
# The point of this script is repeatability: performance work needs the *same*
# machine configuration on every run, and typing into a serial console by hand
# is neither repeatable nor scriptable. It drives QEMU's stdio serial through a
# FIFO, waits for the shell prompt before sending anything (data typed before
# the shell exists is simply lost), and tears the VM down on any exit path.
#
# Usage:
#   scripts/qemu-bench.sh [-o OUTFILE] [-t TIMEOUT] [-s SMP] [-m MEM] CMD [CMD...]
#
#   -o OUTFILE   where to write the captured console log (default: a temp file)
#   -t TIMEOUT   seconds to allow for the whole run (default 900)
#   -s SMP       guest CPU count (default 4)
#   -m MEM       guest RAM (default 4G)
#
# Each CMD is sent as one line to the guest shell. The script waits for the
# prompt to come back between commands, so a long-running command does not have
# its successor typed over the top of it.
#
# Requires `make -C zCore build` to have produced the ESP first; see
# docs/README-performance.md.
#
# NOTE: this runs QEMU **without** KVM acceleration (there is usually no
# /dev/kvm in a build container), i.e. under TCG emulation. Absolute numbers are
# therefore several times worse than the same kernel on real hardware. They are
# still directly comparable *between runs*, and comparable against Linux booted
# with scripts/qemu-linux-bench.sh, which uses the same emulation.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ESP_DIR="$ROOT/target/x86_64/release/esp"
OVMF="$ROOT/rboot/OVMF.fd"

OUTFILE=""
TIMEOUT=900
SMP=4
MEM=4G

while getopts "o:t:s:m:" opt; do
    case "$opt" in
        o) OUTFILE="$OPTARG" ;;
        t) TIMEOUT="$OPTARG" ;;
        s) SMP="$OPTARG" ;;
        m) MEM="$OPTARG" ;;
        *) echo "usage: $0 [-o OUT] [-t TIMEOUT] [-s SMP] [-m MEM] CMD..." >&2; exit 2 ;;
    esac
done
shift $((OPTIND - 1))

if [ $# -eq 0 ]; then
    echo "$0: no commands given" >&2
    exit 2
fi

[ -d "$ESP_DIR/EFI" ] || {
    echo "$0: no ESP at $ESP_DIR — run: make -C zCore build MODE=release LINUX=1" >&2
    exit 1
}

WORK="$(mktemp -d)"
[ -n "$OUTFILE" ] || OUTFILE="$WORK/console.log"
FIFO="$WORK/stdin.fifo"
ESP_IMG="$WORK/esp.img"
mkfifo "$FIFO"

QEMU_PID=""
cleanup() {
    [ -n "$QEMU_PID" ] && kill "$QEMU_PID" 2>/dev/null
    exec 3>&- 2>/dev/null
    rm -rf "$WORK" 2>/dev/null
}
trap cleanup EXIT INT TERM

# QEMU's built-in `fat:rw:` directory emulation is FAT16 and caps at ~504 MiB,
# which the baked-in userland exceeds. Build a real FAT32 ESP image, exactly as
# zCore/Makefile does for `make run`.
esp_mb=$(du -sm "$ESP_DIR/EFI" | cut -f1)
esp_mb=$((esp_mb + 128))
dd if=/dev/zero of="$ESP_IMG" bs=1M count="$esp_mb" status=none
mkfs.vfat -F 32 "$ESP_IMG" >/dev/null
mmd -i "$ESP_IMG" ::/EFI ::/EFI/Boot ::/EFI/zCore
mcopy -i "$ESP_IMG" -s "$ESP_DIR/EFI" ::/

# Match zCore/Makefile's qemu_opts for x86_64, minus the graphics and minus KVM.
# `-serial mon:stdio` is the console we drive; the net device is kept because
# the kernel brings it up during boot either way and removing it would change
# what we are measuring.
qemu-system-x86_64 \
    -smp "$SMP" \
    -machine q35 \
    -cpu Haswell,+smap,-check,-fsgsbase \
    -m "$MEM" \
    -serial mon:stdio \
    -drive format=raw,if=pflash,readonly=on,file="$OVMF" \
    -drive format=raw,file="$ESP_IMG" \
    -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0 -device usb-tablet,bus=xhci.0 \
    -nic none \
    -display none \
    -no-reboot \
    < "$FIFO" > "$OUTFILE" 2>&1 &
QEMU_PID=$!

# Hold the write end open for the life of the run; closing it would send EOF to
# the guest console and kill the shell after the first command.
exec 3>"$FIFO"

deadline=$(( $(date +%s) + TIMEOUT ))

# Wait for `pattern` to appear in the log at least `count` times.
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

# The busybox prompt. Sending anything before it appears is lost: the console
# has no reader yet.
PROMPT='/ #'
wait_for "$PROMPT" 1 || exit 1

# Marker echoed after each command so we can tell "the command finished" from
# "the prompt happened to appear inside the command's own output".
#
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
