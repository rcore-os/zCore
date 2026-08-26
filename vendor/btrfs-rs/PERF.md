# btrfs driver — performance & stress suite

`tests/performance.rs` is a benchmark + correctness harness for the eclipse
btrfs filesystem driver. Every scenario verifies data integrity (so a
correctness regression surfaces as a failed assertion, not a silent bad
number) and, when `btrfs-progs` is available, the resulting image is
cross-validated with `btrfs check`.

## Running

```sh
# from the repo root
scripts/btrfs-bench.sh                       # release build, default scale
BTRFS_BENCH_SCALE=4 scripts/btrfs-bench.sh   # 4x heavier workloads
scripts/btrfs-bench.sh perf_large_file_sequential   # a single scenario

# or directly
cd vendor/btrfs-rs
cargo test --release --features std --test performance -- --nocapture --test-threads=1
```

Always run with `--release`: the driver checksums and reserialises 16 KiB tree
blocks constantly, which a debug build slows down by ~2x and skews every
number.

## Scenarios

| Test | What it stresses |
|------|------------------|
| `perf_large_file_sequential` | streaming write + read throughput of one big file |
| `perf_fragmented_read` | sequential read over many scattered small extents (extent-list scan) |
| `perf_large_file_random` | random 4 KiB read/write over a large file |
| `perf_many_files_one_dir` | create / lookup / readdir / unlink of thousands of files in one directory |
| `perf_lookup_scaling` | lookup cost as a directory grows (must stay ~O(log n)) |
| `perf_fragmentation_scaling` | allocation throughput as free space fragments |
| `perf_rangemap_query_scaling` | free-space query cost vs unrelated fragment count (regression guard) |
| `perf_deep_tree` | building and walking a deeply nested directory tree |
| `perf_small_files_throughput` | write throughput of many medium (8 KiB) files |
| `perf_truncate_churn` | repeated grow/shrink truncation + hole reads |

Each run prints throughput plus an instrumented device-I/O line
(`dev reads/writes`, bytes, syncs) so write amplification and reads-per-op are
visible.

## Findings & fixes

- **Free-space queries were O(n) in total fragment count.**
  `RangeMap::total_free_in` / `largest_in` scanned the whole free map up to
  `hi` instead of only the queried block group's ranges. Because
  `meta_free()` (and thus `ensure_metadata_space`) runs on *every* mutation,
  this degraded to O(n²) as free space fragmented. Fixed by iterating only the
  ranges that can overlap `[lo, hi)` (`ranges_overlapping`). The
  `perf_rangemap_query_scaling` guard pins this: a top-of-space query stays
  ~55-85 ns from 2k to 100k fragments, versus a 69x blow-up
  (3.8 µs → 266 µs) before the fix.

- **Read throughput was being mismeasured.** An earlier harness draft verified
  data byte-by-byte (regenerating an LCG stream) *inside* the timed region,
  reporting ~43 MiB/s for a read that actually runs at multiple GiB/s.
  Verification now happens outside the timed window.

## Known characteristics (not yet changed)

- **Metadata write amplification.** With `SUPERBLOCK_COMMIT_INTERVAL = 32`,
  hot tree blocks (directory leaf, tree roots, extent tree) are re-flushed on
  every periodic superblock commit, so a batch of 8000 tiny files writes
  ~164 MiB of metadata for ~160 KiB of data. This is a durability/throughput
  trade-off; the instrumented `dev writes` line in
  `perf_many_files_one_dir` tracks it.
