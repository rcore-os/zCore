zCore Zircon-mode prebuilt - all three architectures from the same Fuchsia
snapshot (main branch, built 2026-09-03, source under
out/bringup_with_tests.<arch>-release, product bringup_with_tests.<arch>
--release).

Each prebuilt/zircon/<arch>/ contains:
  userboot.so    Zircon first user process (Rust userboot, unstripped)
  userboot-test.so
                 Zircon test userboot selected by core-tests.zbi
  libzircon.so   vDSO (user.basic_<cpu>-shared/libzircon.so.debug)
  bringup.zbi    product bootfs only (KERNEL/CMDLINE stripped)
  core-tests.zbi bootable Zircon core test image

See SHA256SUMS for hashes.
