# hunter — Eclipse OS in-kernel security subsystem

`hunter` is a small in-kernel **security solution** for Eclipse OS that combines
an **LSM-style enforcement layer** (mandatory policy checks at well-defined
kernel hook points) with a **behavioural intrusion-detection system** (IDS).
The kernel calls into a handful of hooks; hunter consults its policy engine,
runs anomaly heuristics, and records every decision in a forensic event log
that userspace can read at `/proc/hunter`.

The crate lives at [`hunter/`](../hunter) and is `#![no_std]`. It has no hard
dependency on `kernel-hal`: the kernel injects a monotonic clock at boot via
`hunter::set_time_source`, keeping the crate low in the build graph and unit
testable on the host.

## Hooks

| Hook                       | Kernel call site                         | Domain          |
|----------------------------|------------------------------------------|-----------------|
| `check_syscall`            | `linux-syscall` dispatch                 | seccomp + IDS   |
| `check_elf_binary`         | `sys_execve`                             | exec integrity  |
| `check_mmap`               | `sys_mmap`                               | W^X memory      |
| `check_mprotect`           | `sys_mprotect`                           | W^X memory      |
| `task_exit`                | `sys_exit_group`                         | state cleanup   |
| `init` / `set_time_source` | `zCore` boot (`primary_main`)            | bring-up        |

## Enforcement domains and modes

There are three independently-tunable enforcement domains, each with its own
`Mode` so the subsystem can be rolled out audit-first and tightened per-domain:

- **`Off`** — domain disabled (no checks, no logging).
- **`Report`** — log the violation but allow the action (audit / IDS mode).
- **`Enforce`** — log the violation and block the action.

| Domain    | What it governs                                   | Default   |
|-----------|---------------------------------------------------|-----------|
| `syscall` | Per-process syscall whitelists (lightweight seccomp) | `Enforce` |
| `wx`      | Write-xor-execute memory (`mmap` / `mprotect`)    | `Report`  |
| `exec`    | Executable path policy (untrusted/world-writable) | `Report`  |
| `anomaly` | Block (vs. only log) detected floods / fork bombs | `Report`  |

The defaults are deliberately conservative. The syscall domain is `Enforce`
because whitelists are opt-in per process (no policy registered ⇒ permissive),
so it cannot break a process that didn't ask for filtering. The W^X, exec-path
and anomaly domains default to `Report` because real dynamic linkers / JITs
transiently create `W+X` mappings and the base system legitimately execs from a
variety of paths — blocking by default would risk breaking userspace. Operators
opt into `Enforce` per domain (`policy::set_wx_mode`, `set_exec_mode`,
`set_anomaly_mode`). The control plane can be sealed **tighten-only**
(`policy::seal_tighten_only`) so that, after boot, no domain can be relaxed —
only moved towards stricter enforcement.

### Hardening (v0.3.0)

A multi-agent adversarial red-team (see `docs/hunter-hardening.md`) drove these
defenses, all preserving the conservative-default contract:

- **W^X is structural, not per-call.** File-backed `mmap` is capped to a
  W^X-preserving permission *ceiling* (no execute on writable file pages, no
  write on executable ones) instead of the old blanket `RXW`, and a per-process
  *ever-writable region* map catches the two-step `mmap(W)`→`mprotect(X)`
  bypass. Under enforcement, `mprotect` no longer swallows a failed narrowing.
- **Exec gate hardened.** Full ELF `e_ident`/`e_type`/`e_machine` validation
  (foreign-arch images rejected); `#!` scripts recognised as valid; the integrity
  check reads the *same VMO bytes* that get mapped (no TOCTOU); `/proc/*/fd/*`
  magic-links and path-traversal treated as untrusted; the dynamic linker and
  shebang interpreter audited through the same gate.
- **Forensics tamper-evident.** Severity-segregated rings so a flood of benign
  events cannot evict `Warning`/`Critical` evidence; per-severity drop counters;
  an optional durable sink streams high-severity events to the kernel log.
- **State lifecycle fixed.** Cleanup runs from the central `Process::terminate`
  (every exit path, not just `exit_group`); per-pid maps are LRU-capped; the
  monotonic clock is sealed and the IDS window has a count backstop so a frozen
  clock cannot silence detection.
- **Per-architecture syscall tables** (x86_64 + asm-generic for aarch64/riscv64)
  and a **system-wide fork-rate** signal for distributed fork bombs.

## Trusted-program allowlist + blacklist (application control)

The `exec` domain combines a **learning allowlist** (a whitelist that never
denies) with an operator **blacklist** (the only hard deny), plus the existing
location denylist. Order of precedence at every `execve` — and for the dynamic
linker and `#!` interpreters, which pass through the same gate:

1. **Blacklist** — a program matching `/etc/hunter/blacklist` is always blocked
   (`EACCES`) while the `exec` domain is active. This is the deny half.
2. **Learning (trust-on-first-use)** — when enabled, a *safe* program (a valid
   ELF/script, not blacklisted, not from a world-writable location such as
   `/tmp`, `/var/tmp`, `/dev/shm` or a `/proc/*/fd/*` magic-link) is
   automatically added to the allowlist and **allowed without denial**. Merely
   relative paths (e.g. a package manager exec'ing `lib/apk/.../busybox`) are
   *not* treated as unsafe, so they are learned rather than warned about.
3. **Denylist + allowlist** — anything left is checked against the untrusted
   locations and, if the allowlist is active, the trusted set. Under `Report`
   this only logs; under `Enforce` a non-trusted program is blocked.

Canonicalization runs first, so a traversal like `/bin/../opt/evil` cannot
masquerade as a trusted program.

### `/etc/hunter/` configuration

At boot the kernel **reads** (never writes) two optional newline-delimited
files from the root filesystem and enables learning:

| File                      | Effect                                         |
|---------------------------|------------------------------------------------|
| `/etc/hunter/whitelist`   | trusted programs that may always run           |
| `/etc/hunter/blacklist`   | denied programs that must never run            |

Each non-empty, non-`#` line is one entry; a trailing `/` makes it a directory
prefix, otherwise it is an exact program path. Missing files are fine (the
lists stay empty). Example `/etc/hunter/whitelist`:

```
# trusted directories
/bin/
/usr/bin/
# one exact extra binary
/opt/myapp/bin/agent
```

### Persistence of learned programs

Learned entries live in kernel memory and are surfaced at `/proc/hunter` under
a `learned-programs (N):` block. The kernel deliberately does **not** write the
filesystem; a userspace helper is expected to read that block and append new
programs to `/etc/hunter/whitelist`, so the learned set survives a reboot.

### Programmatic control

```rust
use hunter::{policy, Mode};

policy::set_exec_learning(true);                              // whitelist learns, never denies
policy::add_blacklisted_exec_prefix(String::from("/tmp/"));  // hard-deny a location
policy::add_trusted_exec_path(String::from("/srv/agent"));   // pre-trust a program
policy::set_exec_mode(Mode::Enforce);                        // also deny non-trusted execs
```

## Intrusion detection (heuristics)

Two signals are produced on the syscall hot path, *after* the policy check so
they only ever observe calls that were allowed to run:

1. **Sensitive-syscall watch** — constant-time classification of each syscall
   against a small table of security-relevant operations (kernel module
   loading, `ptrace`, `bpf`, credential changes, namespace escapes, `kexec`,
   cross-process writes, …). Matches are recorded as audit events; they are
   never blocked here unless the optional privileged-deny latch is engaged. The
   table is selected per architecture (x86_64, and asm-generic for
   aarch64/riscv64).

2. **Rate anomalies** — cheap per-process sliding-window counters that flag
   syscall **floods** (possible DoS) and **fork bombs**, plus a system-wide
   fork-rate signal for *distributed* fork bombs. Each anomaly is reported at
   most once per window to avoid log storms; under the `anomaly` domain's
   `Enforce` mode the offending syscall is denied. Toggle with
   `heuristics::set_anomaly_detection`.

## `/proc/hunter`

Reading `/proc/hunter` renders a live report: a status header (per-domain modes,
event counters, active syscall policies) followed by the recent event ring.
Example:

```
hunter security subsystem v0.4.0
enforcement: syscall=enforce wx=report exec=report anomaly=report
events: total=3 blocked=0 warnings=1 reported=1 critical=0 dropped=0 critical_dropped=0
active syscall policies: 0
trusted-program allowlist: inactive (0 entries)

recent events (oldest first):
[     0] +         0.000s pid=0     INFO   SYSTEM    INIT: hunter security subsystem v0.3.0 initialized (...)
[     1] +         4.512s pid=142   NOTICE PRIVILEGE WATCH: sensitive syscall #101 (ptrace)
[     2] +         5.001s pid=142   WARN   ANOMALY   WARNING: possible fork bomb: >200 clone/fork within 1000ms
```

## Programmatic configuration

```rust
use hunter::{policy, Mode};

// Restrict a process to a syscall whitelist (a lightweight seccomp).
policy::register_policy(pid, vec![/* allowed syscall numbers */]);

// Tighten W^X to active blocking.
policy::set_wx_mode(Mode::Enforce);

// Treat execution from world-writable paths as fatal.
policy::set_exec_mode(Mode::Enforce);
policy::add_untrusted_exec_prefix(String::from("/run/user/"));
```
