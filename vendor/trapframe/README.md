# trapframe-rs

[![Crate](https://img.shields.io/crates/v/trapframe.svg)](https://crates.io/crates/trapframe)
[![Docs](https://docs.rs/trapframe/badge.svg)](https://docs.rs/trapframe)
[![Actions Status](https://github.com/rcore-os/trapframe-rs/actions/workflows/main.yml/badge.svg?branch=main)](https://github.com/rcore-os/trapframe-rs/actions/workflows/main.yml?query=branch%3Amain)

`trapframe-rs` makes it easy to cross the boundary between kernel space and
user space. It packages the architecture-specific register save, privilege
transition, and trap return sequence into a kernel-side function call.

From the kernel's point of view, running user code looks like this:

```text
prepare UserContext -> context.run() -> user code executes
                              ^              |
                              |              | syscall, exception, or interrupt
                              +--------------+
                         context.run() returns
```

When `run()` returns, the same `UserContext` contains the user's latest
registers and trap state. The kernel can inspect the event, update registers,
and call `run()` again to resume execution.

`UserContextWithExtensions` adds architecture-specific floating-point/vector state
without changing the layout of the original `UserContext`: x87/SSE on x86-64,
FP/SIMD on AArch64, and F/D plus 128-bit V registers on RISC-V 64.
The original `UserContext` path does not access floating-point, SIMD, or vector
registers; use the extended type when those registers must be preserved.

The crate is `no_std` and supports x86-64, AArch64, RISC-V 32/64, and
little-endian MIPS.

## Bare-metal usage

Initialize the architecture's trap entry once, construct a user context, and
run it from the kernel:

```rust
use trapframe::{GeneralRegs, UserContext};

unsafe {
    // Installs architecture-specific exception and syscall entry points.
    trapframe::init();
}

let mut context = UserContext {
    general: GeneralRegs {
        // x86-64 example; register names vary by architecture.
        rip: 0x1000,
        rsp: 0x10_000,
        ..Default::default()
    },
    ..Default::default()
};

loop {
    context.run();

    match context.trap_num {
        3 => {
            // Breakpoint: advance past the instruction and resume.
            context.general.rip += 1;
        }
        0x100 => {
            let syscall_number = context.get_syscall_num();
            let arguments = context.get_syscall_args();
            let result = handle_syscall(syscall_number, arguments);
            context.set_syscall_ret(result);
        }
        trap => panic!("unhandled user trap: {trap:#x}"),
    }
}
```

The precise fields that report a trap are architecture-specific. See the
[API documentation](https://docs.rs/trapframe) for the target you are building.

## Kernel traps

On bare-metal targets, define the `trap_handler` symbol expected by the assembly
entry code. The handler receives a mutable architecture-specific `TrapFrame`:

```rust
use trapframe::TrapFrame;

#[unsafe(no_mangle)]
extern "sysv64" fn trap_handler(frame: &mut TrapFrame) {
    log::trace!("kernel trap: {frame:#x?}");
    // Inspect or modify the saved registers before returning from the trap.
}
```

The required ABI is architecture-specific; the other architectures use
`extern "C"`.

## Same-privilege function-call mode

Linux and macOS builds provide `UserContext::run_fncall`. This mode performs the
same register-context handoff inside a normal process, without changing CPU
privilege levels. A cooperating user payload replaces its syscall instruction
with a call to `syscall_fn_entry`, which returns control to `run_fncall`.

This is useful for testing a userspace runtime or embedding it in a host process
while keeping the kernel-facing control flow.

## Supported targets

| Architecture | Targets | Execution mode |
| --- | --- | --- |
| x86-64 | `x86_64-unknown-none`, `x86_64-unknown-uefi` | User/kernel transition |
| x86-64 | `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin` | Same-privilege function call |
| AArch64 | `aarch64-unknown-none-softfloat` | User/kernel transition |
| AArch64 | `aarch64-unknown-linux-gnu` | Same-privilege function call |
| RISC-V 32 | `riscv32imac-unknown-none-elf` | User/kernel transition |
| RISC-V 64 | `riscv64imac-unknown-none-elf` | User/kernel transition |
| MIPS little-endian | `mipsel-unknown-none` with nightly `build-std` | User/kernel transition |

## Features

- `ioport_bitmap`: adds an x86-64 TSS I/O-permission bitmap. It requires a
  contiguous allocation of approximately 64 KiB.

## Examples

- [x86-64 UEFI kernel](./examples/uefi)
- [RISC-V kernel](./examples/riscv)
- [MIPS little-endian kernel](./examples/mipsel)

## How the x86-64 transition works

![x86-64 control flow](./docs/x86_64.svg)
