#!/usr/bin/env bash
# build-mirror.sh — Genera un mirror local FIRMADO con paquetes apk GRANDES y
# REALES (v3), para estresar el camino RX/TX del driver e1000e bajo QEMU con
# transferencias TCP largas. No requiere acceso a Internet.
#
#   repo/x86_64/APKINDEX.tar.gz      índice v3 firmado (mkndx) — apk lo descarga
#   repo/x86_64/bench-bigNN-*.apk    paquetes grandes y firmados (apk fetch)
#   repo/x86_64/bigfile.bin          fichero grande de alta entropía (wget)
#   repo/x86_64/bigfile.sha256       checksum esperado de bigfile.bin
#   keys/<pub>                       clave pública (el guest la confía vía HTTP)
#
# El objetivo es reproducir `apk fetch` del paquete más grande de Alpine
# (llvm22-libs, ~37 MB) que se atasca en hardware real: aquí servimos paquetes
# de tamaño configurable (por defecto 2 × 64 MiB = 128 MiB) y el guest los
# descarga en bucle durante periodos largos, verificando firma e integridad.
#
# Variables de entorno:
#   NUM_BIGPKGS   nº de paquetes grandes a generar      (def. 2)
#   BIGPKG_MB     tamaño de la carga de cada paquete MiB (def. 64)
#   BIGFILE_MB    tamaño de bigfile.bin para wget   MiB  (def. 64)
#   NUM_RECORDS   paquetes dummy extra para engordar el índice (def. 2000)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$REPO_ROOT/local-mirror"
ROOT="$REPO_ROOT"
APK="$ROOT/tools/apk/apk-x86_64.static"
KEY="$HERE/keys/eclipse-bench@local-1.rsa"
PUB="eclipse-bench@local-1.rsa.pub"

NUM_BIGPKGS="${NUM_BIGPKGS:-2}"
BIGPKG_MB="${BIGPKG_MB:-64}"
BIGFILE_MB="${BIGFILE_MB:-64}"
NUM_RECORDS="${NUM_RECORDS:-2000}"

mkdir -p "$HERE/keys"
# Clave de firma (privada gitignored bajo local-mirror/)
if [ ! -f "$KEY" ]; then
  echo "[mirror] generando clave de firma..."
  openssl genrsa -out "$KEY" 2048 2>/dev/null
  openssl rsa -in "$KEY" -pubout -out "$HERE/keys/$PUB" 2>/dev/null
fi
# La pública la confía el guest: (1) vía rootfs (prebuilt/alpine-apk-keys) y
# (2) servida por HTTP para que bench.sh la coloque en /etc/apk/keys en runtime.
mkdir -p "$ROOT/prebuilt/alpine-apk-keys"
cp -f "$HERE/keys/$PUB" "$ROOT/prebuilt/alpine-apk-keys/$PUB"

REPO="$HERE/repo/x86_64"
rm -rf "$HERE/repo"
mkdir -p "$REPO"
KEYSDIR="$HERE/trust"; rm -rf "$KEYSDIR"; mkdir -p "$KEYSDIR"
cp -f "$HERE/keys/$PUB" "$KEYSDIR/$PUB"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# --- Paquetes grandes y reales (v3, firmados) -------------------------------
# Cada paquete lleva una carga de alta entropía de BIGPKG_MB MiB. apk fetch
# descarga el .apk completo y verifica su firma -> si el driver pierde/corrompe
# bytes, apk falla; si el camino es correcto, descarga OK.
PKG_NAMES=""
i=0
while [ "$i" -lt "$NUM_BIGPKGS" ]; do
  name="bench-big$(printf '%02d' "$i")"
  br="$WORK/br-$name"
  mkdir -p "$br/usr/share/$name"
  echo "[mirror] generando $name (${BIGPKG_MB} MiB de carga)..."
  head -c "$((BIGPKG_MB*1024*1024))" /dev/urandom > "$br/usr/share/$name/data.bin"
  "$APK" mkpkg \
    --info "name:$name" --info "version:1.0.0-r0" --info "arch:x86_64" \
    --info "description:Eclipse e1000e bench large package $i" \
    --info "license:MIT" --info "origin:$name" --info "url:https://eclipse.local/" \
    --files "$br" --sign-key "$KEY" \
    -o "$REPO/$name-1.0.0-r0.apk" >/dev/null
  echo "[mirror]   -> $(stat -c%s "$REPO/$name-1.0.0-r0.apk") bytes"
  PKG_NAMES="$PKG_NAMES $name"
  i=$((i+1))
done

# --- Paquetes dummy pequeños para engordar el índice -------------------------
# (apk update/fetch descarga el índice antes de los paquetes; un índice grande
# añade una transferencia TCP de varios MB más al inicio de cada corrida.)
j=0
while [ "$j" -lt "$NUM_RECORDS" ]; do
  name="benchpkg$(printf '%06d' "$j")"
  br="$WORK/br-dummy"
  rm -rf "$br"; mkdir -p "$br/usr/share/doc/$name"
  # contenido único por paquete (checksum real distinto en el índice)
  head -c 256 /dev/urandom > "$br/usr/share/doc/$name/README"
  "$APK" mkpkg \
    --info "name:$name" --info "version:1.0.0-r0" --info "arch:x86_64" \
    --info "description:dummy $j" --info "license:MIT" --info "origin:$name" \
    --files "$br" --sign-key "$KEY" \
    -o "$REPO/$name-1.0.0-r0.apk" >/dev/null
  j=$((j+1))
  [ "$((j % 500))" -eq 0 ] && echo "[mirror]   dummy $j/$NUM_RECORDS"
done

# --- Índice v3 firmado, publicado como APKINDEX.tar.gz ----------------------
echo "[mirror] generando índice v3 (mkndx) sobre $(ls "$REPO"/*.apk | wc -l) paquetes..."
"$APK" mkndx --keys-dir "$KEYSDIR" \
  --description "Eclipse e1000e bench mirror" \
  --sign-key "$KEY" \
  -o "$REPO/APKINDEX.tar.gz" "$REPO"/*.apk >/dev/null
echo "[mirror] APKINDEX.tar.gz: $(stat -c%s "$REPO/APKINDEX.tar.gz") bytes"

# --- bigfile para la ruta wget (descarga cruda, sin apk) --------------------
echo "[mirror] generando bigfile.bin (${BIGFILE_MB} MiB)..."
head -c "$((BIGFILE_MB*1024*1024))" /dev/urandom > "$REPO/bigfile.bin"
( cd "$REPO" && sha256sum bigfile.bin > bigfile.sha256 )

# --- Resumen para el runner / bench.sh --------------------------------------
echo "[mirror] paquetes grandes:${PKG_NAMES}"
echo "[mirror] bigfile.sha256: $(cat "$REPO/bigfile.sha256")"
{
  echo "PUB=$PUB"
  echo "BIG_PKGS=\"${PKG_NAMES# }\""
  echo "BIGPKG_MB=$BIGPKG_MB"
  echo "BIGFILE_MB=$BIGFILE_MB"
  echo "BIGFILE_SHA=$(awk '{print $1}' "$REPO/bigfile.sha256")"
  echo "BIGFILE_SZ=$(stat -c%s "$REPO/bigfile.bin")"
} > "$HERE/mirror.env"
echo "[mirror] listo en $REPO (env -> $HERE/mirror.env)"
