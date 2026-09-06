#!/bin/sh
# Cargo target runner for tests which execute Fuchsia's AArch64 register ABI.
set -eu
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
codesign --force --sign - --entitlements "$script_dir/macos-entitlements.plist" "$1"
exec "$@"
