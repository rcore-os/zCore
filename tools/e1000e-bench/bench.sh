#!/bin/busybox sh
# bench.sh — init del guest para el banco de pruebas del driver e1000e.
#
# Ejercita el camino RX/TX del driver con DOS cargas durante un periodo largo:
#   1) `apk fetch` de paquetes GRANDES y firmados (reproduce el atasco de
#      llvm22-libs en hardware real) -> verifica firma + descarga completa.
#   2) `wget` crudo de bigfile.bin -> verifica tamaño + sha256.
#
# Repite BENCH_N veces (def. 64). Marcadores BENCH_* a serie para el runner.
PATH=/bin:/sbin:/usr/bin:/usr/sbin
export PATH
B=/bin/busybox
$B mount -t proc proc /proc 2>/dev/null
$B mount -t sysfs sysfs /sys 2>/dev/null
$B mount -t tmpfs tmpfs /tmp 2>/dev/null

echo "==== ECLIPSE E1000E BENCH START ===="

# --- red ---
IF=""
for cand in $($B ls /sys/class/net 2>/dev/null); do
  [ "$cand" = "lo" ] && continue; IF="$cand"; break
done
[ -z "$IF" ] && IF=eth0
echo "BENCH_IFACE=$IF"
$B ip link set lo up 2>/dev/null
$B ip link set "$IF" up 2>/dev/null
$B ip addr add 10.0.2.15/24 dev "$IF" 2>/dev/null
$B ip route add default via 10.0.2.2 2>/dev/null

MIRROR=http://10.0.2.2:8080/repo
BASE=http://10.0.2.2:8080

# --- confiar la clave del mirror (servida por HTTP) + repos ---
$B mkdir -p /etc/apk/keys /etc/apk
PUB=eclipse-bench@local-1.rsa.pub
$B timeout 30 $B wget -q -O "/etc/apk/keys/$PUB" "$BASE/keys/$PUB" 2>/dev/null
echo "$MIRROR" > /etc/apk/repositories

# apk real (apk-tools static); el rootfs lo coloca en /bin/apk
APK=/bin/apk
[ -x "$APK" ] || APK=/usr/bin/apk

# paquetes grandes a descargar (override por kernel cmdline env BENCH_PKGS)
BIG_PKGS="${BENCH_PKGS:-bench-big00 bench-big01}"
BIGURL="$MIRROR/x86_64/bigfile.bin"

# sha/tamaño esperados de bigfile.bin: del entorno o, si no, del propio mirror.
EXP_SZ="${BENCH_BIGFILE_SZ:-0}"
EXP_SHA="${BENCH_BIGFILE_SHA:-}"
if [ -z "$EXP_SHA" ]; then
  $B timeout 30 $B wget -q -O /tmp/bigsha "$MIRROR/x86_64/bigfile.sha256" 2>/dev/null
  EXP_SHA=$($B awk '{print $1}' /tmp/bigsha 2>/dev/null)
fi
echo "BENCH_EXPECT sha=$EXP_SHA size=$EXP_SZ pkgs=$BIG_PKGS iters=${BENCH_N:-64}"

AN=${BENCH_N:-64}
i=1
apk_ok=0; apk_fail=0
wg_ok=0; wg_trunc=0; wg_corrupt=0; wg_hang=0
while [ "$i" -le "$AN" ]; do
  # ---- 1) wget crudo de bigfile.bin: UN solo stream TCP grande, el análogo
  #         fiel de la descarga de llvm22-libs que se atasca en hardware real.
  #         Se hace PRIMERO para aislar el camino de transferencia larga del
  #         comportamiento multi-conexión de apk. ----
  $B rm -f /tmp/big
  $B timeout 180 $B wget -q -O /tmp/big "$BIGURL"
  wrc=$?
  SZ=$($B wc -c < /tmp/big 2>/dev/null); [ -z "$SZ" ] && SZ=0
  SHA=$($B sha256sum /tmp/big 2>/dev/null | $B awk '{print $1}')
  if [ "$wrc" = "143" ] || [ "$wrc" = "124" ]; then
    wg_hang=$((wg_hang+1)); echo "BENCH_WGET $i HANG wrc=$wrc size=$SZ/$EXP_SZ"
  elif [ "$EXP_SZ" != "0" ] && [ "$SZ" != "$EXP_SZ" ]; then
    wg_trunc=$((wg_trunc+1)); echo "BENCH_WGET $i TRUNC wrc=$wrc size=$SZ/$EXP_SZ"
  elif [ -n "$EXP_SHA" ] && [ "$SHA" != "$EXP_SHA" ]; then
    wg_corrupt=$((wg_corrupt+1)); echo "BENCH_WGET $i CORRUPT size=$SZ sha=$SHA"
  else
    wg_ok=$((wg_ok+1)); echo "BENCH_WGET $i OK size=$SZ"
  fi

  # ---- 2) apk fetch de paquetes grandes (firma verificada, multi-conexión) ----
  if [ -x "$APK" ] && [ -n "$BIG_PKGS" ]; then
    $B rm -rf /tmp/fetch; $B mkdir -p /tmp/fetch
    $B timeout 180 "$APK" fetch --no-cache -o /tmp/fetch $BIG_PKGS > /tmp/apk.log 2>&1
    arc=$?
    got=$($B ls /tmp/fetch/*.apk 2>/dev/null | $B wc -l)
    want=$(echo $BIG_PKGS | $B wc -w)
    if [ "$arc" = "0" ] && [ "$got" = "$want" ]; then
      apk_ok=$((apk_ok+1)); echo "BENCH_APK $i OK pkgs=$got/$want"
    else
      apk_fail=$((apk_fail+1))
      echo "BENCH_APK $i FAIL rc=$arc pkgs=$got/$want :: $($B tail -1 /tmp/apk.log 2>/dev/null)"
    fi
  fi

  echo "BENCH_PROGRESS $i/$AN wget_ok=$wg_ok apk_ok=$apk_ok apk_fail=$apk_fail"
  i=$((i+1))
done
echo "BENCH_APK_SUMMARY ok=$apk_ok fail=$apk_fail total=$AN"
echo "BENCH_WGET_SUMMARY ok=$wg_ok trunc=$wg_trunc corrupt=$wg_corrupt hang=$wg_hang total=$AN"
echo "==== ECLIPSE E1000E BENCH END ===="
$B sync; $B sleep 1
$B poweroff -f 2>/dev/null
while true; do $B sleep 5; done
