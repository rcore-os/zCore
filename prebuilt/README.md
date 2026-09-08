# Zircon runtime prebuilts

Download the runtime artifacts with `make zircon-init` (or `cargo zircon-init`).
The installer uses the pinned GitHub Release
[`prebuilt-260903`](https://github.com/rcore-os/zCore/releases/tag/prebuilt-260903),
verifies the archive SHA-256 and this directory's `SHA256SUMS`, then installs
the files into the ignored `prebuilt/zircon/` directory. It requires `wget`,
`tar` with xz support, and `shasum` (available on macOS and Ubuntu).

The release contains the Fuchsia main snapshot built on 2026-09-03 for x64,
arm64 and riscv64: upstream userboot/test-userboot, vDSO, bringup and core-test
images, plus separate LibOS vDSOs for x64 and arm64. The arm64 user binaries
and core-test image use 16 KiB ELF segment alignment for Apple Silicon hosts.
Only the LibOS vDSO is patched; see `scripts/gen-prebuilt.sh`.

Binary artifacts belong in GitHub Releases, not Git. When publishing an update,
use a new release tag or asset name, update the URL, cache filename and archive
digest in `xtask/src/main.rs`, and update `prebuilt/SHA256SUMS`. Avoid replacing
the pinned asset in place. CI installs the same release with `make zircon-init`.
