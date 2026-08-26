#!/usr/bin/env bash
# run-bench.sh — Levanta el mirror HTTP local (servidor python en un proceso) y
# arranca Eclipse OS en QEMU (headless, TCG) con el driver e1000e + user-net,
# capturando la serie. Resume los marcadores BENCH_* del guest.
#
# Uso: bash tools/e1000e-bench/run-bench.sh [TIMEOUT_SEG]
#   TIMEOUT_SEG  tiempo máx. de la corrida QEMU (def. 1800 = 30 min)
#   BENCH_N      nº de iteraciones de descarga en el guest (def. 64)
set -u
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$REPO_ROOT/local-mirror"
ROOT="$REPO_ROOT"
# El kernel + ESP se generan con `cargo image` / `make -C zCore build`.
ESP="${ESP:-$ROOT/target/x86_64/release/esp}"
[ -d "$ESP" ] || ESP="$ROOT/zCore/target/x86_64/release/esp"
OVMF="$ROOT/rboot/OVMF.fd"
TIMEOUT="${1:-1800}"
PORT="${PORT:-8080}"
BENCH_N="${BENCH_N:-64}"
SERIAL="/tmp/eclipse-serial.log"
PCAP="/tmp/eclipse.pcap"

rm -f "$SERIAL" "$PCAP"

# Datos del mirror (sha/tamaño esperados, lista de paquetes grandes)
[ -f "$HERE/mirror.env" ] && . "$HERE/mirror.env"
BIG_PKGS="${BIG_PKGS:-bench-big00 bench-big01}"
BIGFILE_SHA="${BIGFILE_SHA:-}"
BIGFILE_SZ="${BIGFILE_SZ:-0}"

# 1. Mirror HTTP en 0.0.0.0:PORT (alcanzable desde el guest como 10.0.2.2:PORT)
pkill -f "http.server $PORT" 2>/dev/null || true
( cd "$HERE" && python3 -m http.server "$PORT" --bind 0.0.0.0 >/tmp/mirror-httpd.log 2>&1 & echo $! > /tmp/mirror-httpd.pid )
# Esperar a que el listener esté arriba sin usar `sleep` (curl con reintentos).
curl -s --retry 20 --retry-connrefused --retry-delay 1 --max-time 5 \
  -o /dev/null "http://127.0.0.1:$PORT/repo/x86_64/APKINDEX.tar.gz" || true
echo "[bench] mirror servido en 0.0.0.0:$PORT (pid $(cat /tmp/mirror-httpd.pid 2>/dev/null))"
echo "[bench] paquetes grandes: $BIG_PKGS"

# Pasar parámetros al guest por kernel cmdline (env BENCH_*).
CMDLINE="LOG=warn:console.shell=true:virtcon.disable=true:ROOT=/dev/vda:ROOTPROC=/bin/busybox?sh?/bench.sh"
CMDLINE="$CMDLINE:BENCH_N=$BENCH_N:BENCH_PKGS=${BIG_PKGS// /?}:BENCH_BIGFILE_SHA=$BIGFILE_SHA:BENCH_BIGFILE_SZ=$BIGFILE_SZ"
echo "[bench] (recuerda: la cmdline del guest la fija 'make build'; ver README)"

# 2. QEMU headless. e1000e + user net (SLIRP). Sin KVM (TCG).
echo "[bench] arrancando QEMU (timeout ${TIMEOUT}s, TCG, BENCH_N=$BENCH_N)..."
timeout "${TIMEOUT}" qemu-system-x86_64 \
    -smp 4 \
    -machine q35 \
    -cpu Haswell,+smap,-check,-fsgsbase \
    -m 2G \
    -display none \
    -serial "file:$SERIAL" \
    -drive "format=raw,if=pflash,readonly=on,file=$OVMF" \
    -drive "format=raw,file=fat:rw:$ESP" \
    -nic none \
    -device qemu-xhci,id=xhci \
    -netdev user,id=net1 \
    -device e1000e,netdev=net1 \
    -object "filter-dump,id=f0,netdev=net1,file=$PCAP" \
    -no-reboot
QEMU_RC=$?
echo "[bench] QEMU terminó rc=$QEMU_RC"

# 3. Parar mirror
[ -f /tmp/mirror-httpd.pid ] && kill "$(cat /tmp/mirror-httpd.pid)" 2>/dev/null

# 4. Resumen
echo "=================== RESUMEN BENCH ==================="
grep -aE "BENCH_APK_SUMMARY|BENCH_WGET_SUMMARY|BENCH_APK .*FAIL|BENCH_WGET .*(HANG|TRUNC|CORRUPT)|tx_drop=|watchdog|link UP|link DOWN" "$SERIAL" 2>/dev/null | tail -40
echo "----------------------------------------------------"
APKSUM=$(grep -a "BENCH_APK_SUMMARY" "$SERIAL" 2>/dev/null | tail -1)
WGSUM=$(grep -a "BENCH_WGET_SUMMARY" "$SERIAL" 2>/dev/null | tail -1)
echo "apk  : ${APKSUM:-<sin marcador>}"
echo "wget : ${WGSUM:-<sin marcador>}"
TXDROP=$(grep -a "tx_drop=" "$SERIAL" 2>/dev/null | tail -1 | sed 's/.*tx_drop=\([0-9]*\).*/\1/')
echo "tx_drop (último watchdog): ${TXDROP:-<sin marcador>}"
echo "serial log       : $SERIAL"
echo "pcap             : $PCAP"
# PASS si no hay fallos apk ni anomalías wget.
if echo "$APKSUM" | grep -q "fail=0" && echo "$WGSUM" | grep -qE "trunc=0 corrupt=0 hang=0"; then
  echo "RESULTADO: PASS"
else
  echo "RESULTADO: FAIL/INCOMPLETO"
fi
