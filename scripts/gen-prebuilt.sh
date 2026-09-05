#!/bin/sh
# Build only the LibOS vDSO from a configured Fuchsia checkout.
#
# Userboot and ZBI artifacts must remain the unmodified upstream builds.  Run
# this script from the Fuchsia source root, optionally setting BUILD_DIR and
# OUTDIR when they differ from `fx get-build-dir` and `zcore_prebuilt`.

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PATCH_FILE="$SCRIPT_DIR/zircon-libos.patch"
FX=${FX:-./scripts/fx}
ARCH=${1:-x64}
OUTDIR=${OUTDIR:-zcore_prebuilt}
BUILD_DIR=${BUILD_DIR:-$("$FX" get-build-dir)}

case "$ARCH" in
    x64|arm64) ;;
    *)
        echo "unsupported architecture: $ARCH" >&2
        exit 2
        ;;
esac

revert_patch() {
    patch -p1 -R < "$PATCH_FILE"
}

patch -p1 < "$PATCH_FILE"
trap revert_patch EXIT HUP INT TERM

"$FX" --dir "$BUILD_DIR" build --no-checks \
    --toolchain="//build/toolchain/zircon:user.basic_${ARCH}-shared" \
    //zircon/kernel/lib/userabi/vdso:libzircon

mkdir -p "$OUTDIR"
cp "$BUILD_DIR/user.basic_${ARCH}-shared/libzircon.so.debug" \
    "$OUTDIR/libzircon-libos.so"

echo "generated $OUTDIR/libzircon-libos.so"
