# Security & Buffer-Overflow Review — zCore fork (j4flmao/zCore)

**Branch:** master @ b385e77d · **Toolchain:** nightly-2026-09-01 · **Env:** WSL2 (Ubuntu) /mnt/d/zCore
**Date:** 2026-09-03 · **Method:** j4flmao skills (dev-loop-security-auditor, dev-loop-code-review) + manual unsafe audit + QEMU/libos runtime A/B testing

---

## Summary

Applied two **defensive, logic-preserving** security fixes (bounded length checks, no panic paths) focused on the user↔kernel syscall trust boundary — the highest-risk surface in the codebase. Both were verified at runtime via the **libos** Zircon and Linux ABIs (real syscall execution under QEMU-free hosted zCore), including an **A/B comparison against the unmodified baseline** to prove no regression and no newly-introduced crash/panic.

**Scope of change:** 2 files. **Logic changed:** none.

---

## Changes Applied

### 1. `kernel-hal/src/common/user.rs` — bounded C-string / argv scans `[MUST]`

Two genuine **out-of-bounds read** bugs on the user/kernel boundary. Both were unbounded loops that walk user-pointed memory with no stop condition, so a malformed user structure (no NUL terminator) reads arbitrarily far past the buffer.

- **`as_c_str()` (user.rs:206)** — original scanned `(0usize..).find(|&i| *ptr.add(i) == 0).unwrap()` with **no upper bound** until it found a NUL, and `.unwrap()`'d (panics if never found). Used by every file-path syscall (`open`, `stat`, `execve`, …). Cap added at **1 MiB** and a `InvalidLength` error returned instead of a panic. Valid paths/strings are far below 1 MiB → no behavior change.
- **`read_cstring_array()` (user.rs:216)** — original `loop { pptr.read() }` walked the **argv/envp pointer array** with **no cardinality bound**. Cap at **65536** strings with `InvalidLength` on overflow. Used by `execve` argv/envp.

### 2. `zircon-syscall/src/channel.rs` — `ProcArgs` header size guard + no-panic cmdline `[MUST]`

- **`hack_core_tests()` (channel.rs:349)** — reinterpreted `&mut Vec<u8>` as a 28-byte `ProcArgs` struct via `&mut *(as_mut_ptr() as *mut ProcArgs)`. If the message buffer was shorter than `size_of::<ProcArgs>()`, this **formed an out-of-bounds reference** (UB). Added a `data.len() >= size_of::<ProcArgs>()` guard; on a short buffer we bail instead of forming the invalid reference.
- **`userboot` cmdline (channel.rs:326)** — replaced `core::str::from_utf8(data).unwrap()` (a **panic** on non-UTF-8 user cmdline) with a non-panicking `match`, scoped locally to that branch.

> **Regression caught & fixed:** an initial version that hoisted `from_utf8(data)` to the top of `hack_core_tests` caused `ChannelTest.CallBytesFitIsOk` (and other channel tests) to **segfault**. That regression was reproduced, root-caused, and fixed by keeping the `from_utf8` scoped to its original branch. Final version passes the full channel A/B suite.

---

## Runtime Verification (A/B against unmodified baseline)

Environment: hosted **libos** zCore (builds & exercises the real syscall path). Each result compared to baseline by stashing changes, rebuilding, and re-running.

### Zircon ABI — `zcore --features "zircon libos"` (`core-tests.zbi`)

| Test | Baseline | With changes |
|------|----------|--------------|
| ChannelTest.CallBytesFitIsOk | OK | **OK** |
| ChannelTest.CallHandleAndBytesFitsIsOk | OK | **OK** |
| ChannelTest.CallDeadlineExceededReturnsTimedOut | OK | **OK** |
| ChannelTest.CallNullptrNumBytesIsInvalidArgs | OK | **OK** |
| ChannelTest.CallConsumesHandlesOnSuccess | OK | **OK** |
| ChannelTest.CallResponseBiggerThanRdNumBytesReturnsBufferTooSmall | OK | **OK** |
| C11ThreadTest.CreateAndVerifyThreadHandle | OK | **OK** |
| C11ThreadTest.LongNameSucceeds | OK | **OK** |
| C11ThreadTest.ThreadLocalErrno | OK | **OK** |
| C11MutexTest.InitalizeLocalMutex / TimeoutElapsed / StaticInitalizerSameBytesAsAuto | OK | **OK** |
| Bti.Clone / Create / GetInfoTest / PinContigFlag / PinContiguous / Resize | OK | **OK** |
| ChannelTest.CallPendingTransactionsUseDifferentIds | FAILED (pre-existing) | FAILED (identical) |
| Bti.NoDelayedUnpin | FAILED (pre-existing) | FAILED (identical) |
| ChannelTest.CallNullptrNumBytesInvalidArgs | FAILED (pre-existing) | FAILED (identical) |

### Linux ABI — `zcore --features "linux libos"` (busybox)

| Command | Result |
|---------|--------|
| `/bin/busybox echo HELLO_WORLD` | OK (no crash/panic) |
| `/bin/busybox ls /bin` | OK |
| `/bin/busybox env` | OK |
| `/bin/busybox pwd` | OK |

- **No panic / no segfault / no crash** in any passing case.
- Pre-existing failures (marked `FAILED` in the project's own test manifest, incl. `CallPendingTransactionsUseDifferentIds` and `Bti.NoDelayedUnpin`) remain identically failing on baseline and with-changes — **not introduced by this work**.
- `cargo clippy -p zcore --features "linux libos"` and `"zircon libos"` → **0 warnings, 0 errors**.
- Full workspace `cargo check --workspace --features "linux libos"` → clean.

---

## Findings NOT changed (documented; changing risks behavior/regression)

These were audited but intentionally **left untouched** to honor the "no logic change / no crash" constraint. Each is a lower-confidence or higher-risk change that requires per-architecture verification.

### [SHOULD] `linux-object/src/loader/abi.rs:76-77` — uninitialized stack buffer
`Vec::with_capacity(0x4000)` + `set_len(0x4000)` marks uninitialized memory as initialized, then bytes are copied to the user bootstrap stack. The used region is always written, but the gap can expose stale allocator bytes (minor info disclosure). Zero-filling changes exact stack bytes — left as-is; recommend `vec![0u8; 0x4000]` in a follow-up with arm-level re-test.

### [SHOULD] User-pointer struct reinterpretations (no alignment/size validation)
- `linux-object/src/thread.rs:188`, `process.rs:248` — user raw pointers cast to `SigInfo`/`UserContext`/`AtomicI32`.
- `linux-object/src/net/{udp.rs:241, netlink.rs:64}` — user pointers to `ArpReq`/headers.
- `zircon-object/src/debuglog.rs:99,117` — buffer↔`DlogHeader` transmute.
These rely on syscall-layer length assumptions. Hardening requires aligning/size checks that change error behavior → deferred.

### [CONSIDER] `ipc/{channel,socket,fifo,eventpair}.rs` — peer written via aliased `Arc` pointer
`&mut *(Arc::as_ptr(&end0) as *mut …)` writes the `.peer` through an aliased mutable reference (UB-adjacent pattern). Refactoring to interior mutability / safe mutation is a substantial change touching all IPC object init — deferred.

### [CONSIDER] `exception.rs:112` / `paged.rs:144` transmute_copy+forget
Layout-dependent serialization / refcount extraction. Internal, stable layouts; deferred.

### [CONSIDER] `loader/src/zircon.rs:364` and global allocators (`memory.rs`, `memory_x86_64.rs`)
`syscall_args` reads `regs.rsp`/`rsp+8` on libos for 7th/8th stack args (required by System V ABI — correct, not a bug). Global allocators use `NonNull::new_unchecked` — standard for allocator impls; deferred.

---

## Repo/Environment setup (completed)
- Added `upstream` → `https://github.com/rcore-os/zCore.git`
- Initialized submodules: `rboot`, `tests` (zcore-tests), `libc-test`
- Verified WSL2 (Ubuntu) build pipeline, nightly-2026-09-01, downloaded zircon core-tests ZBI + linux libos rootfs.

## Next steps (optional, not done)
- `cargo audit` / `cargo deny` dependency/CVE scan.
- Enabled bare-metal (x86_64/riscv64/aarch64) clippy for the changed files.
- Apply the documented [SHOULD]/[CONSIDER] items with per-arch runtime re-test.
