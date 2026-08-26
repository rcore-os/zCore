# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.9.0] - 2022-02-26

- **[Breaking]** Fix dependencies and asm macros for new nightly.

## [0.8.0] - 2021-07-26

- **[Breaking]** Fix dependencies for new nightly.

## [0.7.0] - 2021-02-13

- **[Breaking]** Fix dependencies for new nightly.

## [0.6.0] - 2020-10-01

- **[Breaking]** Fix dependencies for new nightly.

## [0.5.1] - 2020-07-17

- Change IDT gates on x86_64, enable INT3 and INTO from ring3.

## [0.5.0] - 2020-07-15

- Add support for mipsel.
- Add IO port bitmap on x86_64.
- Fix sp and tpidr in TrapFrame on aarch64.

## [0.4.3] - 2020-06-26

- Fix syntax error on aarch64.

## [0.4.2] - 2020-06-26

- Remove dependency of `cortex-a` and `riscv` crate.

## [0.4.1] - 2020-06-26

- Add function call switching for x86_64 (Linux + macOS) and aarch64 (Linux).

## [0.4.0] - 2020-06-24

- **[Breaking]** Change riscv `trap_handler` interface.
- Add support for aarch64.

## [0.3.0] - 2020-06-12

- **[Breaking]** Fix and support new `asm!` syntax of latest nightly.

## [0.2.0] - 2020-05-17

- **[Breaking]** Remove vector / floating registers.

## [0.1.7] - 2020-04-13

- Fix `MXCSR` register initial value.

## [0.1.6] - 2020-04-06

- Support lazy restore vector registers.

## [0.1.5] - 2020-04-01

- Remove dependency of `rdfsbase` instructions.

## [0.1.4] - 2020-03-25

- Fix breakpoint handling on example.
- Fix kernel trap stack alignment.

## [0.1.3] - 2020-03-04

- Fix build on macOS.

## [0.1.2] - 2020-02-27

- Fix saving `FSBASE`.
- Fix TSS stack 16 bytes alignment.

## [0.1.1] - 2020-02-11

- Fix loading CS segment before `iret`.

## [0.1.0] - 2020-02-05

- Support x86_64 and riscv32/64.
