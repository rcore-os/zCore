#!/usr/bin/env bash
#
# Run the btrfs driver performance & stress suite.
#
# Exercises the eclipse btrfs filesystem driver across large/small files, many
# files per directory, deep trees, random I/O, metadata churn and free-space
# fragmentation, printing throughput and device-I/O numbers for each scenario.
# When btrfs-progs is installed the resulting images are cross-validated with
# `btrfs check`.
#
# Usage:
#   scripts/btrfs-bench.sh                 # release build, default scale
#   BTRFS_BENCH_SCALE=4 scripts/btrfs-bench.sh   # heavier workloads
#   scripts/btrfs-bench.sh --debug         # debug build (faster compile)
#
set -euo pipefail

cd "$(dirname "$0")/../vendor/btrfs-rs"

PROFILE="--release"
if [[ "${1:-}" == "--debug" ]]; then
    PROFILE=""
    shift || true
fi

exec cargo test $PROFILE --features std --test performance -- \
    --nocapture --test-threads=1 "$@"
