// eclipse-bench — CPU / memory / syscall / VM / scheduler / disk / process
// benchmark for Eclipse OS.
//
// It is deliberately dependency-free (POSIX + libc only) and statically linked,
// so it can be dropped straight into the rootfs and run from the shell. Every
// micro-benchmark is *time-bounded* (it runs for a target wall-clock budget and
// counts how much work it completed) so the whole suite finishes in well under a
// minute even on a slow USB stick, and adapts automatically to fast (QEMU) vs
// slow (real disk) machines.
//
// Build (musl, static):
//     x86_64-linux-musl-gcc -O2 -static -pthread -o eclipse-bench eclipse-bench.c
// then copy `eclipse-bench` into the rootfs and run:
//     ./eclipse-bench [DIR] [DISK_MB] [MEM_MB]
//
// DIR     directory for the disk tests — MUST live on the filesystem you want to
//         measure (the btrfs/ext2 root), NOT a tmpfs like /tmp or /run, or the
//         "disk" numbers will just measure RAM. Default: current directory.
// DISK_MB size of the disk test file in MiB (default 32).
// MEM_MB  size of the memory working set in MiB (default 32).
//
// Options (before the positional arguments):
//     --only SECTION   run one section: cpu, mem, syscall, vm, sched, smp,
//                      disk, proc
//     --quick          shorter time budgets (rough numbers, ~3x faster)
//     --budget MS      per-measurement wall-clock budget (default 200 ms for the
//                      small probes, 400 ms for the streaming ones)
//     --max MS         hard ceiling per measurement (default 20 s)
//
// Every time-bounded probe also collects at least MIN_SAMPLES samples even if
// that overruns its budget, so a slow kernel yields fewer-but-real numbers
// instead of a number derived from three or four samples. A slower kernel
// therefore makes the *run* longer, not the numbers worse — give the harness a
// correspondingly larger timeout.
//
// ---------------------------------------------------------------------------
// READ THIS BEFORE TRUSTING A NUMBER
// ---------------------------------------------------------------------------
//
// Every line is tagged with what it actually measures:
//
//   [user]   Runs entirely in userspace on already-faulted memory. The kernel
//            is not involved, so this number is a property of the CPU and the
//            compiler — NOT of the operating system. Two different OSes on the
//            same machine MUST produce the same figure; if they do not, the
//            difference is frequency scaling, not kernel quality. These lines
//            can never show that an OS is fast. They are here only to
//            establish the clock the kernel lines are measured against.
//
//   [kernel] Dominated by kernel code paths. This is where an OS is fast or
//            slow, and where a gap against Linux is real.
//
// The suite used to report only [user] figures plus `getpid()`, `fork`, and
// disk throughput. That is why it read as "on par with Linux" while the system
// did not feel like it: the things that decide perceived speed — wake-up
// latency, context-switch cost, page-fault cost, and how any of it behaves when
// more than one thing wants the CPU — were not measured at all. Every
// [user] benchmark also runs *alone* on an otherwise idle machine, which is the
// one condition under which a scheduler cannot be caught being slow.
//
// The SCHEDULER section is the one to watch. `wake latency under load` in
// particular: it is the delay between a task becoming runnable and it actually
// running while the CPUs are busy, and it is what a shell, a compositor or an
// editor spends its life waiting on.
//
// For a real comparison, run *this same binary* on Linux on the *same machine*
// and diff the output. The RATIOS section at the end is designed to survive
// even when you cannot: it reports kernel costs relative to this machine's own
// measured CPU speed, so a slow VM does not disguise a slow kernel.

#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <time.h>
#include <errno.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <sys/auxv.h>
#include <sys/socket.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/time.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>

// ---------------------------------------------------------------------------
// Timing + anti-optimization helpers
// ---------------------------------------------------------------------------

static uint64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

// A volatile sink the loops feed into so the compiler can't elide them.
static volatile uint64_t g_sink;

// Per-microbench wall-clock budgets. `--quick` scales them down, `--budget MS`
// sets them explicitly.
static uint64_t g_budget_ns = 400000000ull;  // 0.4 s — the classic sections
static uint64_t g_short_ns = 200000000ull;   // 0.2 s — the many small probes

// Minimum samples a time-bounded measurement must collect before it may stop,
// regardless of the wall-clock budget.
//
// A pure time budget silently degrades as the operation gets slower: at 46 ms
// per `fork+exec`, a 0.2 s budget buys FOUR samples, and four samples of
// anything on a loaded emulated machine is not a measurement. The floor makes a
// slow path take longer rather than report a number nobody should trust — which
// is the right trade for a benchmark whose whole job is telling slow from fast.
// It also means a run gets *longer* as the kernel gets slower, so budget the
// harness timeout accordingly (scripts/qemu-bench.sh -t).
#define MIN_SAMPLES 24
// Ceiling on that generosity: without it a pathologically slow operation could
// hold the suite for hours. Reaching it is reported, not hidden.
static uint64_t g_max_ns = 20000000000ull; // 20 s per measurement

#define NA (-1.0)

// Run `fn(n)` with a growing `n` until one call lasts >= `budget_ns`, then
// return the achieved rate in operations/second. `fn` must return a value
// derived from its work (fed into g_sink) so it isn't optimized away.
static double timed_oprate(uint64_t (*fn)(uint64_t), uint64_t budget_ns) {
    uint64_t n = 1u << 16;
    for (;;) {
        uint64_t t0 = now_ns();
        uint64_t r = fn(n);
        uint64_t t1 = now_ns();
        g_sink += r;
        uint64_t dt = t1 - t0;
        if (dt >= budget_ns)
            return (double)n * 1e9 / (double)dt;
        if (dt < 1000) { // too fast to measure — grow aggressively
            n <<= 3;
            continue;
        }
        // Scale n to land a bit past the budget next time.
        double factor = (double)budget_ns / (double)dt * 1.3;
        uint64_t next = (uint64_t)((double)n * factor);
        n = next > n ? next : n * 2;
    }
}

// Run `fn()` repeatedly and return the mean nanoseconds per call. Stops once
// the wall-clock budget is spent *and* at least MIN_SAMPLES calls have been
// made, or when the hard ceiling is hit. `fn` returns 0 on success and non-zero
// to abort the measurement (the operation is unsupported on this kernel), in
// which case NA is returned.
static double timed_ns_per_op(int (*fn)(void), uint64_t budget_ns) {
    // Warm up once so a lazily-initialised path (first mmap of an arena, first
    // open of a device) is not charged to the measurement.
    if (fn() != 0)
        return NA;
    uint64_t t0 = now_ns(), elapsed = 0, ops = 0;
    // Batch of 1 until we know roughly how slow the operation is: batching 64
    // calls of a 46 ms operation would overshoot the budget by three seconds.
    int batch = 1;
    while (elapsed < budget_ns || ops < MIN_SAMPLES) {
        for (int k = 0; k < batch; k++) {
            if (fn() != 0)
                return NA;
            ops++;
        }
        elapsed = now_ns() - t0;
        if (elapsed >= g_max_ns)
            break;
        // Grow the batch only while calls are cheap enough that the timing
        // overhead would otherwise dominate.
        if (batch < 64 && ops > 0 && elapsed / ops < 100000)
            batch = 64;
    }
    return ops ? (double)elapsed / (double)ops : NA;
}

// ---------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------

static void line(void) {
    printf("----------------------------------------------------------------------\n");
}

// One measured row. `unit` is printed after the value; `hint` is a short note
// (typically a Linux orientation figure). NA prints as "n/a".
static void row(const char *tag, const char *label, double value,
                const char *unit, const char *hint) {
    printf("  %-8s %-28s ", tag, label);
    if (value < 0)
        printf("%12s", "n/a");
    else if (value >= 1000.0)
        printf("%12.0f", value);
    else if (value >= 10.0)
        printf("%12.1f", value);
    else
        printf("%12.2f", value);
    printf(" %-9s", unit);
    if (hint && *hint)
        printf(" %s", hint);
    printf("\n");
}

static void hr_bytes(double bps, char *out, size_t n) {
    const char *u = "B/s";
    double v = bps;
    if (v >= 1e9) { v /= 1e9; u = "GB/s"; }
    else if (v >= 1e6) { v /= 1e6; u = "MB/s"; }
    else if (v >= 1e3) { v /= 1e3; u = "KB/s"; }
    snprintf(out, n, "%.1f %s", v, u);
}

// ---------------------------------------------------------------------------
// CPU  [user]
// ---------------------------------------------------------------------------

// Dependent 64-bit multiply-add chain (a PCG-style LCG). Each iteration depends
// on the previous, so the loop is latency-bound: its rate is ~proportional to
// effective core frequency / IPC and is the cleanest "what clock am I actually
// running at" signal.
static uint64_t cpu_int_chain(uint64_t iters) {
    uint64_t x = 0x9e3779b97f4a7c15ull;
    for (uint64_t i = 0; i < iters; i++)
        x = x * 6364136223846793005ull + 1442695040888963407ull;
    return x;
}

// Independent integer ops across 4 accumulators — measures instruction-level
// throughput (IPC * freq) rather than latency.
static uint64_t cpu_int_tput(uint64_t iters) {
    uint64_t a = 1, b = 2, c = 3, d = 4;
    for (uint64_t i = 0; i < iters; i++) {
        a = a * 2654435761u + 1;
        b = b * 2246822519u + 3;
        c = c * 3266489917u + 5;
        d = d * 668265263u + 7;
    }
    return a ^ b ^ c ^ d;
}

// Dependent double-precision multiply-add chain — float-unit frequency proxy.
static uint64_t cpu_double_chain(uint64_t iters) {
    double f = 1.0000001, a = 1.0000000007, b = 0.0000000003;
    for (uint64_t i = 0; i < iters; i++)
        f = f * a + b;
    return (uint64_t)(f * 1000.0);
}

// ---------------------------------------------------------------------------
// Memory  [user]
// ---------------------------------------------------------------------------

static size_t g_mem_bytes;
static unsigned char *g_mem_src, *g_mem_dst;
static size_t *g_chase; // permuted index cycle for pointer chasing

static uint64_t mem_copy(uint64_t passes) {
    uint64_t bytes = 0;
    for (uint64_t p = 0; p < passes; p++) {
        memcpy(g_mem_dst, g_mem_src, g_mem_bytes);
        bytes += g_mem_bytes;
    }
    return bytes;
}

static uint64_t mem_set(uint64_t passes) {
    uint64_t bytes = 0;
    for (uint64_t p = 0; p < passes; p++) {
        memset(g_mem_dst, (int)(p & 0xff), g_mem_bytes);
        bytes += g_mem_bytes;
    }
    return bytes;
}

// Pointer-chase latency: follow a random cycle so each load depends on the
// previous, defeating prefetch — measures the memory/cache miss latency.
static uint64_t mem_chase(uint64_t hops) {
    size_t i = 0;
    for (uint64_t h = 0; h < hops; h++)
        i = g_chase[i];
    return (uint64_t)i;
}

// Build a single random permutation cycle over `n` slots (Sattolo's algorithm),
// so following g_chase visits every slot exactly once before repeating.
static void build_chase(size_t n) {
    for (size_t i = 0; i < n; i++)
        g_chase[i] = i;
    uint64_t r = 0x243f6a8885a308d3ull;
    for (size_t i = n - 1; i > 0; i--) {
        r = r * 6364136223846793005ull + 1442695040888963407ull;
        size_t j = (size_t)((r >> 11) % i); // 0..i-1
        size_t t = g_chase[i];
        g_chase[i] = g_chase[j];
        g_chase[j] = t;
    }
}

// ---------------------------------------------------------------------------
// Syscall entry cost  [kernel]
// ---------------------------------------------------------------------------
//
// `getpid()` is the floor: trap in, read one field, trap out. Everything else
// here adds one specific kernel subsystem on top of that floor, so the
// *difference* between a row and the getpid row localises the cost.
//
// `clock_gettime` deserves special attention: on Linux it is served from the
// vDSO and never enters the kernel at all (~25 ns). A figure here in the same
// range as `getpid` means Eclipse is taking a real trap for it — and since
// timestamps are one of the most frequently issued operations in any real
// program (every log line, every timeout, every animation frame), that alone
// is worth a vDSO.

static int g_devnull = -1, g_devzero = -1;

static int sc_getpid(void)  { return getpid() > 0 ? 0 : -1; }

static int sc_clock_gettime(void) {
    struct timespec ts;
    return clock_gettime(CLOCK_MONOTONIC, &ts);
}

// Whether this process was handed a vDSO at all.
//
// The timing above says how expensive a clock read is; this says whether the
// kernel even offered a way to avoid the trap. The two answer different
// questions and the distinction matters when a number fails to move: a missing
// AT_SYSINFO_EHDR means the kernel published nothing, while a present one with
// trap-sized timings means the libc looked and declined — a malformed image, or
// a kernel that mapped it but left it disabled. Guessing between those from a
// timing alone costs a boot cycle each way.
#ifndef AT_SYSINFO_EHDR
#define AT_SYSINFO_EHDR 33
#endif

static const char *vdso_presence(void) {
#ifdef __linux__
    unsigned long base = getauxval(AT_SYSINFO_EHDR);
    if (!base) return "absent (AT_SYSINFO_EHDR not published)";
    // musl resolves the symbol itself and silently falls back if it cannot;
    // reporting the mapping base is enough to separate "no vDSO" from "vDSO
    // present but unused".
    static char buf[64];
    snprintf(buf, sizeof buf, "mapped at %#lx", base);
    return buf;
#else
    return "n/a";
#endif
}

static int sc_clock_gettime_real(void) {
    struct timespec ts;
    return clock_gettime(CLOCK_REALTIME, &ts);
}

static int sc_gettimeofday(void) {
    struct timeval tv;
    return gettimeofday(&tv, NULL);
}

static int sc_time(void) {
    return time(NULL) > 0 ? 0 : -1;
}

static volatile sig_atomic_t g_sig_seen;

static void bench_sig_handler(int sig) {
    (void)sig;
    g_sig_seen = 1;
}

static int sc_signal_self(void) {
    g_sig_seen = 0;
    if (raise(SIGUSR1) != 0)
        return -1;
    // Delivery for a self-raised signal happens before `raise` returns, so an
    // unset flag means the handler never ran and the row must be n/a rather
    // than a suspiciously fast number.
    return g_sig_seen ? 0 : -1;
}

static int sc_read1(void) {
    char c;
    return pread(g_devzero, &c, 1, 0) == 1 ? 0 : -1;
}

static int sc_write1(void) {
    return write(g_devnull, "x", 1) == 1 ? 0 : -1;
}

static int sc_open_close(void) {
    int fd = open("/dev/null", O_WRONLY);
    if (fd < 0)
        return -1;
    close(fd);
    return 0;
}

static int sc_fstat(void) {
    struct stat st;
    return fstat(g_devnull, &st);
}

static int sc_stat_path(void) {
    struct stat st;
    return stat("/dev/null", &st);
}

static int sc_sigprocmask(void) {
    sigset_t set;
    sigemptyset(&set);
    return sigprocmask(SIG_SETMASK, &set, NULL);
}

static int sc_sched_yield(void) { return sched_yield(); }

// ---------------------------------------------------------------------------
// Virtual memory  [kernel]
// ---------------------------------------------------------------------------

#define VM_PAGES 256 // 1 MiB worth of 4 KiB pages per mmap round

static int sc_mmap_munmap(void) {
    void *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED)
        return -1;
    return munmap(p, 4096);
}

static int sc_mprotect(void) {
    static void *p;
    if (!p) {
        p = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p == MAP_FAILED) { p = NULL; return -1; }
    }
    if (mprotect(p, 4096, PROT_READ) != 0)
        return -1;
    return mprotect(p, 4096, PROT_READ | PROT_WRITE);
}

// Minor (demand-zero) fault cost: map a fresh anonymous region and touch one
// byte per page, so every touch is a first-touch fault. Amortises the
// mmap/munmap over VM_PAGES faults, then subtracts nothing — the residual is
// small next to the faults and is disclosed in the label.
static double vm_minor_fault_ns(uint64_t budget_ns) {
    size_t len = (size_t)VM_PAGES * 4096;
    uint64_t t0 = now_ns(), elapsed = 0, faults = 0, mapped_ns = 0;
    while (elapsed < budget_ns) {
        uint64_t m0 = now_ns();
        unsigned char *p = mmap(NULL, len, PROT_READ | PROT_WRITE,
                                MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p == MAP_FAILED)
            return NA;
        uint64_t m1 = now_ns();
        for (int i = 0; i < VM_PAGES; i++)
            p[(size_t)i * 4096] = (unsigned char)i;
        uint64_t m2 = now_ns();
        munmap(p, len);
        faults += VM_PAGES;
        mapped_ns += m2 - m1;
        (void)m0;
        elapsed = now_ns() - t0;
    }
    return faults ? (double)mapped_ns / (double)faults : NA;
}

// Copy-on-write *correctness*: the parent fills a private region with one
// pattern, forks, the child overwrites it with another and exits, and the
// parent then checks its own bytes are untouched — and vice versa.
//
// This is the test that has to pass before any COW speedup means anything. A
// `fork` that shares frames without write-protecting them is *fast* and
// *wrong*: the child's stores land in the parent's memory. Returns 0 on
// success, or the number of the first check that failed.
static int vm_cow_isolation_check(void) {
    size_t pages = 256; // 1 MiB
    size_t len = pages * 4096;
    unsigned char *p = mmap(NULL, len, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED)
        return -1;
    for (size_t i = 0; i < pages; i++)
        p[i * 4096] = 0xA5;
    int fds[2];
    if (pipe(fds) != 0) { munmap(p, len); return -1; }
    pid_t c = fork();
    if (c < 0) { close(fds[0]); close(fds[1]); munmap(p, len); return -1; }
    if (c == 0) {
        close(fds[0]);
        // The child must still see the parent's pre-fork bytes...
        int bad = 0;
        for (size_t i = 0; i < pages; i++)
            if (p[i * 4096] != 0xA5) { bad = 1; break; }
        // ...then overwrite them privately.
        for (size_t i = 0; i < pages; i++)
            p[i * 4096] = 0x5A;
        for (size_t i = 0; i < pages; i++)
            if (p[i * 4096] != 0x5A) { bad = 2; break; }
        ssize_t w = write(fds[1], &bad, sizeof bad);
        (void)w;
        _exit(0);
    }
    close(fds[1]);
    int child_bad = -1;
    ssize_t r = read(fds[0], &child_bad, sizeof child_bad);
    close(fds[0]);
    int st;
    waitpid(c, &st, 0);
    int rc = 0;
    if (r != (ssize_t)sizeof child_bad) rc = 3;
    else if (child_bad != 0) rc = child_bad;
    else {
        // The parent's own bytes must be exactly as it left them.
        for (size_t i = 0; i < pages; i++)
            if (p[i * 4096] != 0xA5) { rc = 4; break; }
    }
    munmap(p, len);
    return rc;
}

// Copy-on-write fault cost: pre-fault a private region in the parent, fork, and
// have the child write one byte per page — every write breaks a COW share. The
// child reports through a pipe. This is the cost every `fork` of a real program
// pays as the child touches its inherited heap and stack.
static double vm_cow_fault_ns(void) {
    size_t pages = 1024; // 4 MiB
    size_t len = pages * 4096;
    unsigned char *p = mmap(NULL, len, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED)
        return NA;
    for (size_t i = 0; i < pages; i++)
        p[i * 4096] = 1; // parent pre-faults, so the child's writes are COW
    int fds[2];
    if (pipe(fds) != 0) { munmap(p, len); return NA; }
    pid_t c = fork();
    if (c < 0) { close(fds[0]); close(fds[1]); munmap(p, len); return NA; }
    if (c == 0) {
        close(fds[0]);
        uint64_t t0 = now_ns();
        for (size_t i = 0; i < pages; i++)
            p[i * 4096] = 2;
        uint64_t dt = now_ns() - t0;
        ssize_t w = write(fds[1], &dt, sizeof dt);
        (void)w;
        _exit(0);
    }
    close(fds[1]);
    uint64_t dt = 0;
    ssize_t r = read(fds[0], &dt, sizeof dt);
    close(fds[0]);
    int st;
    waitpid(c, &st, 0);
    munmap(p, len);
    if (r != (ssize_t)sizeof dt)
        return NA;
    return (double)dt / (double)pages;
}

// ---------------------------------------------------------------------------
// Scheduler / IPC  [kernel]  — the section that decides how the system feels
// ---------------------------------------------------------------------------

// One pipe round trip: write a byte, block until the peer answers. The peer has
// to be woken, scheduled and run for this to complete, so the figure is
// (2 x context switch) + (4 x pipe syscall). It is the single best proxy for
// "how long does anything wait to be given a CPU".

struct pingpong { int a[2], b[2]; };

static int pingpong_open(struct pingpong *pp) {
    if (pipe(pp->a) != 0)
        return -1;
    if (pipe(pp->b) != 0) {
        close(pp->a[0]); close(pp->a[1]);
        return -1;
    }
    return 0;
}

static void pingpong_close(struct pingpong *pp) {
    close(pp->a[0]); close(pp->a[1]);
    close(pp->b[0]); close(pp->b[1]);
}

// Echo loop for the far side: read from `in`, write back to `out`, until EOF.
static void pingpong_echo(int in, int out) {
    char c;
    while (read(in, &c, 1) == 1) {
        if (write(out, &c, 1) != 1)
            break;
    }
}

static void *pingpong_thread(void *arg) {
    struct pingpong *pp = arg;
    pingpong_echo(pp->a[0], pp->b[1]);
    return NULL;
}

// Drive `iters`-bounded round trips over an already-open pair; returns ns per
// round trip, or NA if the peer stopped answering.
static double pingpong_drive(struct pingpong *pp, uint64_t budget_ns) {
    char c = 'x';
    // One untimed round trip so the peer is definitely parked in `read` before
    // the clock starts (otherwise the first iteration measures process startup).
    if (write(pp->a[1], &c, 1) != 1 || read(pp->b[0], &c, 1) != 1)
        return NA;
    uint64_t t0 = now_ns(), elapsed = 0, ops = 0;
    int batch = 1;
    while ((elapsed < budget_ns || ops < MIN_SAMPLES) && elapsed < g_max_ns) {
        for (int k = 0; k < batch; k++) {
            if (write(pp->a[1], &c, 1) != 1)
                return NA;
            if (read(pp->b[0], &c, 1) != 1)
                return NA;
            ops++;
        }
        elapsed = now_ns() - t0;
        // Batch only once a round trip is known to be cheap; at milliseconds
        // per round trip a batch of 32 overshoots the budget many times over.
        if (batch < 32 && ops > 0 && elapsed / ops < 100000)
            batch = 32;
    }
    return ops ? (double)elapsed / (double)ops : NA;
}

static double sched_pipe_rt_proc(uint64_t budget_ns) {
    struct pingpong pp;
    if (pingpong_open(&pp) != 0)
        return NA;
    pid_t c = fork();
    if (c < 0) { pingpong_close(&pp); return NA; }
    if (c == 0) {
        close(pp.a[1]); close(pp.b[0]);
        pingpong_echo(pp.a[0], pp.b[1]);
        _exit(0);
    }
    close(pp.a[0]); close(pp.b[1]);
    double ns = pingpong_drive(&pp, budget_ns);
    close(pp.a[1]); // EOF ends the child's echo loop
    close(pp.b[0]);
    int st;
    waitpid(c, &st, 0);
    return ns;
}

static double sched_pipe_rt_thread(uint64_t budget_ns) {
    struct pingpong pp;
    if (pingpong_open(&pp) != 0)
        return NA;
    pthread_t th;
    if (pthread_create(&th, NULL, pingpong_thread, &pp) != 0) {
        pingpong_close(&pp);
        return NA;
    }
    double ns = pingpong_drive(&pp, budget_ns);
    close(pp.a[1]);
    pthread_join(th, NULL);
    close(pp.a[0]); close(pp.b[0]); close(pp.b[1]);
    return ns;
}

// Thread creation: pthread_create + join of a no-op thread. Everything a
// threaded program pays before its thread runs: kernel thread object, stack
// mapping, TLS setup, wake, and the join handshake on exit.
static void *noop_thread_fn(void *a) { return a; }

static double sched_thread_spawn_ns(uint64_t budget_ns) {
    uint64_t t0 = now_ns(), elapsed = 0, ops = 0;
    while ((elapsed < budget_ns || ops < MIN_SAMPLES) && elapsed < g_max_ns) {
        pthread_t t;
        if (pthread_create(&t, NULL, noop_thread_fn, NULL) != 0)
            return NA;
        pthread_join(t, NULL);
        ops++;
        elapsed = now_ns() - t0;
    }
    return ops ? (double)elapsed / (double)ops : NA;
}

// Futex wake round trip between two threads. This is the primitive under every
// mutex, condvar and `park` in every threaded program -- musl's pthreads are
// futex all the way down -- so its round trip bounds how fast two threads can
// hand work to each other. Distinct from the pipe row: no file descriptors, no
// data copy, just sleep/wake through the kernel.
#define ECL_FUTEX_WAIT 0
#define ECL_FUTEX_WAKE 1

static volatile int g_fx_ping, g_fx_pong;
static volatile int g_fx_stop;

static long futex_op(volatile int *uaddr, int op, int val) {
    return syscall(SYS_futex, uaddr, op, val, NULL, NULL, 0);
}

static void *futex_echo_thread(void *arg) {
    (void)arg;
    for (;;) {
        while (!__atomic_exchange_n(&g_fx_ping, 0, __ATOMIC_ACQ_REL)) {
            if (g_fx_stop)
                return NULL;
            futex_op(&g_fx_ping, ECL_FUTEX_WAIT, 0);
        }
        __atomic_store_n(&g_fx_pong, 1, __ATOMIC_RELEASE);
        futex_op(&g_fx_pong, ECL_FUTEX_WAKE, 1);
    }
}

static double sched_futex_rt_ns(uint64_t budget_ns) {
    g_fx_ping = g_fx_pong = 0;
    g_fx_stop = 0;
    pthread_t t;
    if (pthread_create(&t, NULL, futex_echo_thread, NULL) != 0)
        return NA;
    uint64_t t0 = now_ns(), elapsed = 0, ops = 0;
    while ((elapsed < budget_ns || ops < MIN_SAMPLES) && elapsed < g_max_ns) {
        __atomic_store_n(&g_fx_ping, 1, __ATOMIC_RELEASE);
        futex_op(&g_fx_ping, ECL_FUTEX_WAKE, 1);
        while (!__atomic_exchange_n(&g_fx_pong, 0, __ATOMIC_ACQ_REL))
            futex_op(&g_fx_pong, ECL_FUTEX_WAIT, 0);
        ops++;
        elapsed = now_ns() - t0;
    }
    g_fx_stop = 1;
    futex_op(&g_fx_ping, ECL_FUTEX_WAKE, 1);
    pthread_join(t, NULL);
    return ops ? (double)elapsed / (double)ops : NA;
}

// The same ping-pong as the pipe row, over an AF_UNIX socketpair. Sockets and
// pipes take different kernel paths (socket buffers and their wakeups against
// the pipe machinery), and a desktop is glued together with UNIX sockets --
// Wayland, D-Bus, X11 -- so a slow one is felt even when pipes are fast.
static double sched_socketpair_rt_proc(uint64_t budget_ns) {
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0)
        return NA;
    pid_t c = fork();
    if (c < 0) {
        close(sv[0]); close(sv[1]);
        return NA;
    }
    if (c == 0) {
        close(sv[0]);
        pingpong_echo(sv[1], sv[1]);
        _exit(0);
    }
    close(sv[1]);
    // A socketpair is full duplex: one fd both writes and reads. Dress it as a
    // `pingpong` so the driver (with its warm-up and batching) is shared.
    struct pingpong pp = {{-1, sv[0]}, {sv[0], -1}};
    double ns = pingpong_drive(&pp, budget_ns);
    close(sv[0]); // EOF ends the child's echo loop
    int st;
    waitpid(c, &st, 0);
    return ns;
}

// Pipe throughput with 64 KiB writes: the latency rows move one byte, this
// moves bulk. `cmd | cmd` pipelines and anything that streams through a pipe
// run at this speed, and it exercises a different path than the round trip --
// big copies in and out of the pipe buffer, and how often the reader wakes.
static double sched_pipe_bw_mbs(uint64_t budget_ns) {
    int p[2];
    if (pipe(p) != 0)
        return NA;
    pid_t c = fork();
    if (c < 0) {
        close(p[0]); close(p[1]);
        return NA;
    }
    static char buf[1 << 16];
    if (c == 0) {
        close(p[1]);
        while (read(p[0], buf, sizeof buf) > 0)
            ;
        _exit(0);
    }
    close(p[0]);
    memset(buf, 0x5a, sizeof buf);
    uint64_t t0 = now_ns(), elapsed = 0, bytes = 0;
    while ((elapsed < budget_ns || bytes < (4u << 20)) && elapsed < g_max_ns) {
        if (write(p[1], buf, sizeof buf) != (ssize_t)sizeof buf)
            break;
        bytes += sizeof buf;
        elapsed = now_ns() - t0;
    }
    close(p[1]); // EOF stops the reader
    int st;
    waitpid(c, &st, 0);
    if (!bytes || !elapsed)
        return NA;
    return (double)bytes / ((double)elapsed / 1e9) / 1e6;
}

// Sleep overshoot: ask for `req_us`, measure what you actually got. The excess
// is timer granularity plus the delay between the timer firing and this thread
// being put back on a CPU. Measured twice — idle and under load — because the
// difference between the two IS the interactive latency the user experiences.
//
// Returns the mean and, through `worst`, the largest single overshoot. The
// worst case is the one that matters: a scheduler that usually responds in
// 80 us but occasionally makes you wait out a full 20 ms timeslice is
// experienced as stuttering, and a mean would hide that entirely.
static double sleep_overshoot_us(uint64_t req_us, int rounds, double *worst) {
    struct timespec req = {
        .tv_sec = (time_t)(req_us / 1000000),
        .tv_nsec = (long)((req_us % 1000000) * 1000),
    };
    double total = 0, max = 0;
    int ok = 0;
    for (int i = 0; i < rounds; i++) {
        uint64_t t0 = now_ns();
        if (nanosleep(&req, NULL) != 0 && errno != EINTR)
            return NA;
        uint64_t dt = now_ns() - t0;
        double over = ((double)dt - (double)req_us * 1000.0) / 1000.0;
        if (over < 0)
            over = 0;
        total += over;
        if (over > max)
            max = over;
        ok++;
    }
    if (worst)
        *worst = ok ? max : NA;
    return ok ? total / ok : NA;
}

// Spawn `n` CPU-bound child processes that spin until killed. Returns how many
// actually started; fills `pids`.
static int load_start(pid_t *pids, int n) {
    int started = 0;
    for (int i = 0; i < n; i++) {
        pid_t c = fork();
        if (c < 0)
            break;
        if (c == 0) {
            // Pure userspace spin: no syscalls, so the only way this child ever
            // gives up its CPU is if the kernel takes it away. That is exactly
            // the pressure we want to put on the scheduler.
            volatile uint64_t x = 1;
            for (;;)
                x = x * 6364136223846793005ull + 1442695040888963407ull;
        }
        pids[started++] = c;
    }
    return started;
}

static void load_stop(pid_t *pids, int n) {
    for (int i = 0; i < n; i++)
        kill(pids[i], SIGKILL);
    for (int i = 0; i < n; i++) {
        int st;
        waitpid(pids[i], &st, 0);
    }
}

// ---------------------------------------------------------------------------
// SMP scaling  [kernel]
// ---------------------------------------------------------------------------
//
// N threads each run the same dependent-MAC loop that the CPU section runs
// alone. Perfect scaling means N threads finish N times as much work. Anything
// less is the kernel: lock contention, timer overhead, a scheduler that will
// not spread the threads, or CPUs it never brought online.

// Threads spin on this until every one of them exists, so the measured window
// starts with the full width already running. Without the gate, thread
// creation (which on a loaded emulated machine can take longer than the whole
// budget) ate the measurement: threads created late found the deadline already
// past, did zero iterations, and the section reported 0 Mops/s.
static volatile int g_smp_go;
static volatile uint64_t g_smp_deadline_ns;

struct spin_arg {
    uint64_t iters;
};

static void *spin_thread(void *arg) {
    struct spin_arg *a = arg;
    while (!g_smp_go)
        sched_yield();
    uint64_t x = 0x9e3779b97f4a7c15ull, n = 0;
    while (now_ns() < g_smp_deadline_ns) {
        for (int k = 0; k < 4096; k++)
            x = x * 6364136223846793005ull + 1442695040888963407ull;
        n += 4096;
    }
    g_sink += x;
    a->iters = n;
    return NULL;
}

// Aggregate Mops/s across `n` threads over `budget_ns`. NA if the full width
// could not be started — a narrower run would understate scaling, not measure it.
static double smp_aggregate(int n, uint64_t budget_ns) {
    if (n < 1)
        return NA;
    pthread_t *th = calloc((size_t)n, sizeof *th);
    struct spin_arg *args = calloc((size_t)n, sizeof *args);
    if (!th || !args) { free(th); free(args); return NA; }
    g_smp_go = 0;
    int started = 0;
    for (int i = 0; i < n; i++) {
        args[i].iters = 0;
        if (pthread_create(&th[i], NULL, spin_thread, &args[i]) != 0)
            break;
        started++;
    }
    if (started != n) {
        // Release whatever did start so the joins below cannot hang.
        g_smp_deadline_ns = now_ns();
        g_smp_go = 1;
        for (int i = 0; i < started; i++)
            pthread_join(th[i], NULL);
        free(th);
        free(args);
        return NA;
    }
    uint64_t t0 = now_ns();
    g_smp_deadline_ns = t0 + budget_ns;
    g_smp_go = 1;
    uint64_t total = 0;
    for (int i = 0; i < started; i++) {
        pthread_join(th[i], NULL);
        total += args[i].iters;
    }
    uint64_t dt = now_ns() - t0;
    free(th);
    free(args);
    if (dt == 0 || total == 0)
        return NA;
    return (double)total * 1e9 / (double)dt / 1e6;
}

// ---------------------------------------------------------------------------
// SMP kernel paths  [kernel] — contention, TLB shootdowns, cross-CPU wakes
// ---------------------------------------------------------------------------
//
// The scaling block above runs pure userspace ALU: it proves the CPUs exist
// and the scheduler spreads threads, and nothing else. Everything an SMP
// kernel actually has to get right — syscall entry that does not serialize,
// VM locks that do not collapse under parallel mappers, TLB shootdowns that
// do not stall the world, futex queues under contention, wakes that cross
// CPUs — lives below, measured as x1 against xN so every row carries its own
// baseline.

static volatile int g_smpk_go, g_smpk_stop;

struct smpk_arg {
    uint64_t ops;
    int (*fn)(int);
    int idx;
};

static void *smpk_worker(void *argp) {
    struct smpk_arg *a = argp;
    while (!g_smpk_go)
        sched_yield();
    uint64_t n = 0;
    while (!g_smpk_stop) {
        if (a->fn(a->idx) < 0)
            break;
        n++;
    }
    a->ops = n;
    return NULL;
}

// Aggregate ops/s of `n` workers hammering `fn` for `budget_ns`. NA unless the
// full width started — a narrower run would flatter the contention rows.
static double smpk_rate(int n, int (*fn)(int), uint64_t budget_ns) {
    enum { MAXW = 64 };
    pthread_t th[MAXW];
    struct smpk_arg args[MAXW];
    if (n < 1 || n > MAXW)
        return NA;
    g_smpk_go = 0;
    g_smpk_stop = 0;
    int made = 0;
    for (int i = 0; i < n; i++) {
        args[i].ops = 0;
        args[i].fn = fn;
        args[i].idx = i;
        if (pthread_create(&th[i], NULL, smpk_worker, &args[i]) != 0)
            break;
        made++;
    }
    uint64_t t0 = now_ns();
    g_smpk_go = 1;
    if (made == n) {
        uint64_t ns = budget_ns < g_max_ns ? budget_ns : g_max_ns;
        struct timespec ts = {(time_t)(ns / 1000000000ull),
                              (long)(ns % 1000000000ull)};
        nanosleep(&ts, NULL);
    }
    g_smpk_stop = 1;
    uint64_t total = 0;
    for (int i = 0; i < made; i++) {
        pthread_join(th[i], NULL);
        total += args[i].ops;
    }
    uint64_t el = now_ns() - t0;
    if (made != n || el == 0 || total == 0)
        return NA;
    return (double)total * 1e9 / (double)el;
}

static int smpk_getpid_op(int idx) {
    (void)idx;
    return getpid() > 0 ? 0 : -1;
}

// One op = map 64 KiB, touch it, unmap: the address-space lock and the page
// tables, exercised from every CPU at once.
static int smpk_mmap_op(int idx) {
    (void)idx;
    unsigned char *p = mmap(NULL, 1 << 16, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED)
        return -1;
    p[0] = 1;
    return munmap(p, 1 << 16);
}

// One op = 64 minor faults (256 KiB touched page by page) plus the teardown.
// The frame allocator and fault path under parallel load.
static int smpk_fault_op(int idx) {
    (void)idx;
    size_t len = 64 * 4096;
    unsigned char *p = mmap(NULL, len, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED)
        return -1;
    for (size_t o = 0; o < len; o += 4096)
        p[o] = (unsigned char)o;
    return munmap(p, len);
}

// One shared mutex, zero-length critical section: the futex sleep/wake path at
// its most contended. musl's pthread_mutex is futex all the way down.
static pthread_mutex_t g_smpk_mutex = PTHREAD_MUTEX_INITIALIZER;
static volatile uint64_t g_smpk_mutex_word;

static int smpk_mutex_op(int idx) {
    (void)idx;
    pthread_mutex_lock(&g_smpk_mutex);
    g_smpk_mutex_word++;
    pthread_mutex_unlock(&g_smpk_mutex);
    return 0;
}

// ns per mprotect PTE flip with `peers` sibling threads spinning on other
// CPUs. Each flip must invalidate the sibling CPUs' TLBs; with zero peers the
// kernel may skip idle CPUs entirely, so the DIFFERENCE between the two rows
// is the cross-CPU shootdown cost — IPI round trips and ack waits — isolated
// from the local page-table work.
static volatile int g_smpk_spin_stop;

static void *smpk_spinner(void *arg) {
    (void)arg;
    volatile uint64_t x = 1;
    while (!g_smpk_spin_stop)
        x = x * 6364136223846793005ull + 1442695040888963407ull;
    return NULL;
}

static double smpk_mprotect_ns(int peers, uint64_t budget_ns) {
    enum { MAXP = 63 };
    pthread_t th[MAXP];
    if (peers < 0)
        peers = 0;
    if (peers > MAXP)
        peers = MAXP;
    unsigned char *page = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                               MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (page == MAP_FAILED)
        return NA;
    page[0] = 1; // committed: the flip has a live PTE to change
    g_smpk_spin_stop = 0;
    int made = 0;
    for (int i = 0; i < peers; i++)
        if (pthread_create(&th[i], NULL, smpk_spinner, NULL) == 0)
            made++;
    struct timespec settle = {0, 80 * 1000 * 1000};
    nanosleep(&settle, NULL); // let the spinners actually occupy their CPUs
    uint64_t t0 = now_ns(), elapsed = 0, ops = 0;
    int failed = 0;
    while ((elapsed < budget_ns || ops < MIN_SAMPLES) && elapsed < g_max_ns) {
        if (mprotect(page, 4096, PROT_READ) != 0 ||
            mprotect(page, 4096, PROT_READ | PROT_WRITE) != 0) {
            failed = 1;
            break;
        }
        page[0]++;
        ops += 2;
        elapsed = now_ns() - t0;
    }
    g_smpk_spin_stop = 1;
    for (int i = 0; i < made; i++)
        pthread_join(th[i], NULL);
    munmap(page, 4096);
    if (failed || made != peers || ops == 0)
        return NA;
    return (double)elapsed / (double)ops;
}

// Pipe round trip with both ends pinned: same CPU against adjacent CPUs. The
// same-CPU case is a pure context-switch ping-pong (no IPI, hot cache); the
// cross-CPU case pays the remote wake. The ratio is what moving a wake across
// the machine costs.
static int smpk_pin_self(int cpu) {
    cpu_set_t set;
    CPU_ZERO(&set);
    CPU_SET(cpu, &set);
    return pthread_setaffinity_np(pthread_self(), sizeof set, &set);
}

struct smpk_pinned {
    struct pingpong pp;
    int cpu;
    volatile int pin_failed;
};

static void *smpk_pinned_echo(void *arg) {
    struct smpk_pinned *p = arg;
    if (p->cpu >= 0 && smpk_pin_self(p->cpu) != 0)
        p->pin_failed = 1;
    pingpong_echo(p->pp.a[0], p->pp.b[1]);
    return NULL;
}

static double smpk_pipe_rt_pinned(int cpu_a, int cpu_b, int ncpu,
                                  uint64_t budget_ns) {
    struct smpk_pinned c;
    if (pingpong_open(&c.pp) != 0)
        return NA;
    c.cpu = cpu_b;
    c.pin_failed = 0;
    int self_ok = smpk_pin_self(cpu_a) == 0;
    pthread_t t;
    if (pthread_create(&t, NULL, smpk_pinned_echo, &c) != 0) {
        pingpong_close(&c.pp);
        return NA;
    }
    double ns = self_ok ? pingpong_drive(&c.pp, budget_ns) : NA;
    close(c.pp.a[1]); // EOF ends the echo loop
    pthread_join(t, NULL);
    close(c.pp.a[0]);
    close(c.pp.b[0]);
    close(c.pp.b[1]);
    // Unpin so later sections are not accidentally confined to one CPU.
    cpu_set_t all;
    CPU_ZERO(&all);
    for (int i = 0; i < ncpu && i < CPU_SETSIZE; i++)
        CPU_SET(i, &all);
    pthread_setaffinity_np(pthread_self(), sizeof all, &all);
    if (!self_ok || c.pin_failed)
        return NA;
    return ns;
}

// Aggregate forks/s with `nproc` worker processes forking in parallel: the
// whole copy-on-write machinery — hidden-node creation, mapping walks, the
// family locks — colliding from every CPU at once.
static double smpk_forks_per_s(int nproc, uint64_t budget_ns) {
    enum { MAXF = 64 };
    if (nproc < 1 || nproc > MAXF)
        return NA;
    uint64_t *counts = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                            MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    if (counts == MAP_FAILED)
        return NA;
    memset(counts, 0, 4096);
    uint64_t t0 = now_ns();
    uint64_t deadline = t0 + (budget_ns < g_max_ns ? budget_ns : g_max_ns);
    pid_t kids[MAXF];
    int made = 0;
    for (int i = 0; i < nproc; i++) {
        pid_t c = fork();
        if (c == 0) {
            uint64_t n = 0;
            while (now_ns() < deadline) {
                pid_t g = fork();
                if (g == 0)
                    _exit(0);
                if (g < 0)
                    break;
                int st;
                waitpid(g, &st, 0);
                n++;
            }
            counts[i] = n;
            _exit(0);
        }
        if (c < 0)
            break;
        kids[made++] = c;
    }
    for (int i = 0; i < made; i++) {
        int st;
        waitpid(kids[i], &st, 0);
    }
    uint64_t el = now_ns() - t0;
    uint64_t total = 0;
    for (int i = 0; i < made; i++)
        total += counts[i];
    munmap(counts, 4096);
    if (made != nproc || el == 0 || total == 0)
        return NA;
    return (double)total * 1e9 / (double)el;
}

// Fairness: 2xN identical hogs racing for N CPUs; each counts its progress in
// its own cache line of a shared page. max/min after the window says whether
// the scheduler shares the machine or starves someone — a kernel can post a
// perfect aggregate while one hog gets 10x another's CPU time, and the starved
// one is the interactive shell you are typing into.
static double smpk_fairness_maxmin(int nhogs, uint64_t budget_ns) {
    enum { MAXH = 32, STRIDE = 8 };
    if (nhogs < 2 || nhogs > MAXH)
        return NA;
    volatile uint64_t *counts =
        mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS,
             -1, 0);
    if (counts == MAP_FAILED)
        return NA;
    memset((void *)counts, 0, 4096);
    pid_t kids[MAXH];
    int made = 0;
    for (int i = 0; i < nhogs; i++) {
        pid_t c = fork();
        if (c == 0) {
            volatile uint64_t *mine = &counts[i * STRIDE];
            uint64_t x = 1;
            for (;;) {
                for (int k = 0; k < 2048; k++)
                    x = x * 6364136223846793005ull + 1442695040888963407ull;
                *mine += 1;
            }
        }
        if (c < 0)
            break;
        kids[made++] = c;
    }
    uint64_t ns = budget_ns < g_max_ns ? budget_ns : g_max_ns;
    struct timespec ts = {(time_t)(ns / 1000000000ull),
                          (long)(ns % 1000000000ull)};
    nanosleep(&ts, NULL);
    uint64_t mn = UINT64_MAX, mx = 0;
    for (int i = 0; i < made; i++) {
        uint64_t v = counts[i * STRIDE];
        if (v < mn)
            mn = v;
        if (v > mx)
            mx = v;
    }
    for (int i = 0; i < made; i++)
        kill(kids[i], SIGKILL);
    for (int i = 0; i < made; i++) {
        int st;
        waitpid(kids[i], &st, 0);
    }
    munmap((void *)counts, 4096);
    if (made != nhogs || mn == 0)
        return NA;
    return (double)mx / (double)mn;
}

// ---------------------------------------------------------------------------
// Disk  [kernel]
// ---------------------------------------------------------------------------

static double disk_seq_write(const char *path, size_t bytes, size_t chunk,
                             unsigned char *buf) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return NA;
    uint64_t t0 = now_ns();
    size_t done = 0;
    while (done < bytes) {
        size_t want = bytes - done < chunk ? bytes - done : chunk;
        ssize_t w = write(fd, buf, want);
        if (w <= 0) { close(fd); return NA; }
        done += (size_t)w;
    }
    fsync(fd);
    uint64_t t1 = now_ns();
    close(fd);
    return (double)bytes * 1e9 / (double)(t1 - t0);
}

static double disk_seq_read(const char *path, size_t chunk, unsigned char *buf) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return NA;
    uint64_t t0 = now_ns();
    uint64_t total = 0;
    for (;;) {
        ssize_t r = read(fd, buf, chunk);
        if (r < 0) { close(fd); return NA; }
        if (r == 0) break;
        total += (uint64_t)r;
    }
    uint64_t t1 = now_ns();
    close(fd);
    if (total == 0) return NA;
    return (double)total * 1e9 / (double)(t1 - t0);
}

static double disk_rand_read(const char *path, size_t bytes, uint64_t budget_ns,
                             double *avg_us) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return NA;
    const size_t blk = 4096;
    size_t nblk = bytes / blk;
    if (nblk == 0) { close(fd); return NA; }
    unsigned char b[4096];
    uint64_t r = 0x1234567890abcdefull;
    uint64_t ops = 0, t0 = now_ns(), elapsed = 0;
    while ((elapsed < budget_ns || ops < MIN_SAMPLES) && elapsed < g_max_ns) {
        for (int k = 0; k < 64; k++) {
            r = r * 6364136223846793005ull + 1442695040888963407ull;
            off_t off = (off_t)((r >> 12) % nblk) * (off_t)blk;
            if (pread(fd, b, blk, off) != (ssize_t)blk) { close(fd); return NA; }
            ops++;
        }
        elapsed = now_ns() - t0;
    }
    close(fd);
    double iops = (double)ops * 1e9 / (double)elapsed;
    if (avg_us) *avg_us = (double)elapsed / (double)ops / 1000.0;
    return iops;
}

static double disk_fsync_ms(const char *path, unsigned char *buf) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return NA;
    double best = 1e30;
    for (int i = 0; i < 8; i++) {
        if (write(fd, buf, 4096) != 4096) { close(fd); return NA; }
        uint64_t t0 = now_ns();
        fsync(fd);
        double ms = (double)(now_ns() - t0) / 1e6;
        if (ms < best) best = ms;
    }
    close(fd);
    return best;
}

// Metadata ops: create up to `max` small files in `dir` (time-bounded), then
// stat each, then unlink each. Reports the three rates via out params.
static void disk_metadata(const char *dir, uint64_t budget_ns, int max,
                          double *creates_s, double *stats_s, double *unlinks_s) {
    char path[512];
    *creates_s = *stats_s = *unlinks_s = NA;

    int made = 0;
    uint64_t t0 = now_ns();
    while (made < max && now_ns() - t0 < budget_ns) {
        snprintf(path, sizeof path, "%s/eb_%06d", dir, made);
        int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd < 0) break;
        if (write(fd, "x", 1) != 1) { close(fd); break; }
        close(fd);
        made++;
    }
    uint64_t t1 = now_ns();
    if (made > 0) *creates_s = (double)made * 1e9 / (double)(t1 - t0);

    t0 = now_ns();
    int ok = 0;
    for (int i = 0; i < made; i++) {
        snprintf(path, sizeof path, "%s/eb_%06d", dir, i);
        struct stat st;
        if (stat(path, &st) == 0) ok++;
    }
    t1 = now_ns();
    if (ok > 0) *stats_s = (double)ok * 1e9 / (double)(t1 - t0);

    t0 = now_ns();
    int rm = 0;
    for (int i = 0; i < made; i++) {
        snprintf(path, sizeof path, "%s/eb_%06d", dir, i);
        if (unlink(path) == 0) rm++;
    }
    t1 = now_ns();
    if (rm > 0) *unlinks_s = (double)rm * 1e9 / (double)(t1 - t0);
}

// ---------------------------------------------------------------------------
// Process creation  [kernel]
// ---------------------------------------------------------------------------

// fork + immediate child _exit, with `mib` MiB of pre-faulted private memory
// resident in the parent. Returns ns per fork, or NA if the region cannot be
// allocated.
//
// This is the measurement that tells copy-on-write `fork` from an eager one,
// and it is worth more than any single-size fork number. A COW kernel builds
// the child's address space by sharing frames and write-protecting them, so its
// cost barely moves with the resident set. A kernel that copies every resident
// frame at `fork` time is O(resident): the same shell, the same command, but a
// process holding 100 MiB pays a 100 MiB memcpy every time it forks.
//
// The `COW fault (after fork)` row above cannot reveal this on its own — with an
// eager `fork` the child's pages are already private, so its "COW faults" are
// plain stores and the row reports an implausibly *good* number.
static double proc_fork_resident_ns(size_t mib, uint64_t budget_ns) {
    size_t len = mib * 1024 * 1024;
    unsigned char *p = mmap(NULL, len, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED)
        return NA;
    // Fault every page in so it is genuinely resident, and write a value so no
    // kernel can keep it shared with a global zero page.
    for (size_t i = 0; i < len; i += 4096)
        p[i] = (unsigned char)(i >> 12);
    uint64_t ops = 0, t0 = now_ns(), elapsed = 0;
    while ((elapsed < budget_ns || ops < MIN_SAMPLES) && elapsed < g_max_ns) {
        pid_t c = fork();
        if (c == 0) _exit(0);
        if (c < 0) { munmap(p, len); return NA; }
        int st;
        waitpid(c, &st, 0);
        ops++;
        elapsed = now_ns() - t0;
    }
    munmap(p, len);
    return ops ? (double)elapsed / (double)ops : NA;
}

// fork with `extra` additional mappings present, holding the resident set fixed.
//
// The resident-size probe above answers "does fork copy the pages?". This one
// answers a question it cannot see at all: what does fork cost *per mapping*?
//
// The two are independent, and conflating them hid a real 3x regression. A
// copy-on-write fork stops paying per page but starts paying per mapping — it
// must write-protect each one and, if the kernel shoots down the other CPUs'
// TLBs once per mapping, that is an IPI round trip with an ack spin-wait each
// time. A process with a hundred small mappings then forks far more slowly than
// one with a single large one holding the same bytes, which no per-MiB number
// can express.
//
// Every mapping is one page and is touched once, so the resident set grows by
// `extra` pages -- negligible next to the 1 MiB baseline below, which is there
// precisely so the two runs differ in mapping count and in nothing else. They
// are also deliberately not adjacent: a kernel that merges neighbouring VMAs
// would otherwise collapse them into one and the probe would measure nothing.
// Create `extra` one-page mappings that no kernel can coalesce, and return how
// many were made (pointers in `*spots_out`, to be released with
// `scatter_free`). Reserving one run and punching every other page out of it
// guarantees the gaps without depending on where the kernel would otherwise
// place independent `mmap`s.
static int scatter_mappings(int extra, unsigned char ***spots_out) {
    *spots_out = NULL;
    if (extra <= 0)
        return 0;
    unsigned char **spots = calloc((size_t)extra, sizeof *spots);
    if (!spots)
        return 0;
    size_t run = (size_t)extra * 2 * 4096;
    unsigned char *arena = mmap(NULL, run, PROT_READ | PROT_WRITE,
                                MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (arena == MAP_FAILED) {
        free(spots);
        return 0;
    }
    int made = 0;
    for (int i = 0; i < extra; i++) {
        unsigned char *keep = arena + (size_t)i * 2 * 4096;
        munmap(keep + 4096, 4096);   // the gap that keeps `keep` separate
        keep[0] = (unsigned char)i;  // resident, so it is not merely reserved
        spots[made++] = keep;
    }
    *spots_out = spots;
    return made;
}

static void scatter_free(unsigned char **spots, int n) {
    for (int i = 0; i < n; i++)
        munmap(spots[i], 4096);
    free(spots);
}

static double proc_fork_mappings_ns(int extra, uint64_t budget_ns) {
    const size_t base_len = 1024 * 1024;
    unsigned char *base = mmap(NULL, base_len, PROT_READ | PROT_WRITE,
                               MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (base == MAP_FAILED)
        return NA;
    for (size_t i = 0; i < base_len; i += 4096)
        base[i] = (unsigned char)(i >> 12);

    unsigned char **spots = NULL;
    int made = scatter_mappings(extra, &spots);
    if (extra > 0 && made == 0) {
        munmap(base, base_len);
        return NA;
    }

    uint64_t ops = 0, t0 = now_ns(), elapsed = 0;
    while ((elapsed < budget_ns || ops < MIN_SAMPLES) && elapsed < g_max_ns) {
        pid_t c = fork();
        if (c == 0) _exit(0);
        if (c < 0) break;
        int st;
        waitpid(c, &st, 0);
        ops++;
        elapsed = now_ns() - t0;
    }

    scatter_free(spots, made);
    munmap(base, base_len);
    return ops ? (double)elapsed / (double)ops : NA;
}

// fork + immediate child _exit, parent waits. Returns ns per fork.
static double proc_fork_ns(uint64_t budget_ns) {
    uint64_t ops = 0, t0 = now_ns(), elapsed = 0;
    while ((elapsed < budget_ns || ops < MIN_SAMPLES) && elapsed < g_max_ns) {
        pid_t p = fork();
        if (p == 0) _exit(0);
        if (p < 0) return NA;
        int st;
        waitpid(p, &st, 0);
        ops++;
        elapsed = now_ns() - t0;
    }
    return ops ? (double)elapsed / (double)ops : NA;
}

// fork + execve(self, "--noop") which exits at once, parent waits. Measures the
// full process-replacement cost (address-space teardown + ELF load + setup).
static double proc_fork_exec_ns(uint64_t budget_ns, const char *self) {
    uint64_t ops = 0, t0 = now_ns(), elapsed = 0;
    char *const argv[] = {(char *)self, (char *)"--noop", NULL};
    while ((elapsed < budget_ns || ops < MIN_SAMPLES) && elapsed < g_max_ns) {
        pid_t p = fork();
        if (p == 0) {
            execv(self, argv);
            _exit(127);
        }
        if (p < 0) return NA;
        int st;
        waitpid(p, &st, 0);
        if (!WIFEXITED(st) || WEXITSTATUS(st) != 0) return NA;
        ops++;
        elapsed = now_ns() - t0;
    }
    return ops ? (double)elapsed / (double)ops : NA;
}

// fork + exec a *shell* running a no-op. This is what a script, a Makefile or an
// interactive prompt pays per command: the shell binary is usually dynamically
// linked, so it exercises the loader, the dynamic linker and a real path
// lookup, none of which the static self-exec above touches.
static double proc_spawn_shell_ns(uint64_t budget_ns, const char *sh) {
    uint64_t ops = 0, t0 = now_ns(), elapsed = 0;
    char *const argv[] = {(char *)sh, (char *)"-c", (char *)":", NULL};
    while ((elapsed < budget_ns || ops < MIN_SAMPLES) && elapsed < g_max_ns) {
        pid_t p = fork();
        if (p == 0) {
            execv(sh, argv);
            _exit(127);
        }
        if (p < 0) return NA;
        int st;
        waitpid(p, &st, 0);
        if (!WIFEXITED(st) || WEXITSTATUS(st) == 127) return NA;
        ops++;
        elapsed = now_ns() - t0;
    }
    return ops ? (double)elapsed / (double)ops : NA;
}

static const char *find_shell(void) {
    static const char *candidates[] = {"/bin/sh", "/bin/busybox", "/bin/dash",
                                       "/bin/bash", NULL};
    for (int i = 0; candidates[i]; i++) {
        struct stat st;
        if (stat(candidates[i], &st) == 0 && (st.st_mode & S_IXUSR))
            return candidates[i];
    }
    return NULL;
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

static int want(const char *only, const char *section) {
    return !only || strcmp(only, section) == 0;
}

int main(int argc, char **argv) {
    // Self-exec target for the fork+exec benchmark: exit immediately.
    if (argc > 1 && strcmp(argv[1], "--noop") == 0)
        return 0;

    // Line-buffer stdout. When this runs on a serial console under QEMU, or
    // with its output piped to a file, libc would otherwise pick full
    // buffering and hold everything until the 4 KiB buffer fills — so a run
    // that hangs or panics mid-suite loses the very rows that would say where.
    setvbuf(stdout, NULL, _IOLBF, 0);

    // `--forkloop N MIB`: fork/exit N times with MIB MiB of pre-faulted private
    // memory resident, printing the cost of EVERY iteration as it happens.
    //
    // Not a benchmark — a diagnostic. When `fork` hangs intermittently, waiting
    // for a random hang and then staring at a silent console says nothing about
    // which of the two possible shapes it is, and each attempt costs a full boot.
    // A per-iteration trace answers it in one run:
    //
    //   * iterations getting steadily slower  -> algorithmic. Something is
    //     accumulating per fork (a copy-on-write tree that is not collapsing,
    //     a growing mapping list), and the "hang" is just the curve going
    //     vertical.
    //   * iterations flat, then one never finishes -> a stall. A deadlock, a
    //     lost wakeup, or an unresolvable fault loop, and the iteration number
    //     says how much state it took to get there.
    //
    // The optional third argument adds that many extra one-page mappings, which
    // turns the same trace into the answer to a different question: the
    // benchmark's `fork cost per mapping` row times `fork + exit` together, so a
    // large per-mapping cost could live in either half. Splitting `fork` from
    // `wait` says which — the parent's copy-on-write setup, or the child tearing
    // its address space down again on exit — and those are different bugs in
    // different files.
    //
    // Line-buffered, so the last line printed is the last iteration that
    // completed even if the machine dies mid-fork.
    if (argc > 1 && strcmp(argv[1], "--forkloop") == 0) {
        setvbuf(stdout, NULL, _IOLBF, 0);
        long iters = argc > 2 ? strtol(argv[2], NULL, 10) : 40;
        size_t mib = argc > 3 ? (size_t)strtoul(argv[3], NULL, 10) : 1;
        int maps = argc > 4 ? (int)strtol(argv[4], NULL, 10) : 0;
        size_t len = mib * 1024 * 1024;
        unsigned char *p = NULL;
        if (len) {
            p = mmap(NULL, len, PROT_READ | PROT_WRITE,
                     MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            if (p == MAP_FAILED) {
                printf("forkloop: mmap %zu MiB failed\n", mib);
                return 1;
            }
            for (size_t i = 0; i < len; i += 4096)
                p[i] = (unsigned char)(i >> 12);
        }
        unsigned char **spots = NULL;
        int made = scatter_mappings(maps, &spots);
        printf("forkloop: %ld iterations, %zu MiB resident, %d extra mappings\n",
               iters, mib, made);
        for (long i = 0; i < iters; i++) {
            uint64_t t0 = now_ns();
            pid_t c = fork();
            if (c == 0) _exit(0);
            if (c < 0) { printf("forkloop: fork failed at %ld\n", i); return 1; }
            uint64_t t1 = now_ns();          // fork returned in the parent
            int st;
            waitpid(c, &st, 0);
            uint64_t t2 = now_ns();
            // Split so a stall can be attributed to fork itself versus the
            // child's exit and reaping.
            printf("forkloop %3ld: fork %8.0f us  wait %8.0f us\n", i,
                   (double)(t1 - t0) / 1000.0, (double)(t2 - t1) / 1000.0);
        }
        scatter_free(spots, made);
        printf("forkloop: done\n");
        return 0;
    }

    const char *only = NULL;
    int argi = 1;
    while (argi < argc && argv[argi][0] == '-' && argv[argi][1] == '-') {
        if (strcmp(argv[argi], "--only") == 0 && argi + 1 < argc) {
            only = argv[++argi];
        } else if (strcmp(argv[argi], "--quick") == 0) {
            g_budget_ns = 150000000ull;
            g_short_ns = 60000000ull;
        } else if (strcmp(argv[argi], "--budget") == 0 && argi + 1 < argc) {
            // Per-measurement wall-clock budget in milliseconds. Raise it when
            // the numbers are noisy: every time-bounded probe simply collects
            // more samples.
            uint64_t ms = strtoull(argv[++argi], NULL, 10);
            if (ms < 10) ms = 10;
            g_short_ns = ms * 1000000ull;
            g_budget_ns = g_short_ns * 2;
        } else if (strcmp(argv[argi], "--max") == 0 && argi + 1 < argc) {
            // Ceiling per measurement in milliseconds; raise it if a slow path
            // is being truncated, lower it to bound a run.
            uint64_t ms = strtoull(argv[++argi], NULL, 10);
            if (ms < 100) ms = 100;
            g_max_ns = ms * 1000000ull;
        } else {
            fprintf(stderr,
                    "usage: %s [--only SECTION] [--quick] [--budget MS] [--max MS]"
                    " [DIR] [DISK_MB] [MEM_MB]\n",
                    argv[0]);
            fprintf(stderr, "sections: cpu mem syscall vm sched smp disk proc\n");
            return 2;
        }
        argi++;
    }

    const char *dir = (argc > argi) ? argv[argi] : ".";
    size_t disk_mb = (argc > argi + 1) ? (size_t)strtoul(argv[argi + 1], NULL, 10) : 32;
    size_t mem_mb = (argc > argi + 2) ? (size_t)strtoul(argv[argi + 2], NULL, 10) : 32;
    if (disk_mb < 1) disk_mb = 1;
    if (mem_mb < 1) mem_mb = 1;

    char self[512];
    ssize_t sl = readlink("/proc/self/exe", self, sizeof self - 1);
    if (sl > 0) self[sl] = 0;
    else snprintf(self, sizeof self, "%s", argv[0]);

    long ncpu_l = sysconf(_SC_NPROCESSORS_ONLN);
    int ncpu = (ncpu_l > 0 && ncpu_l < 4096) ? (int)ncpu_l : 2;

    printf("eclipse-bench — CPU / memory / syscall / VM / scheduler / disk / process\n");
    printf("dir=%s  disk=%zu MiB  mem=%zu MiB  cpus=%d\n", dir, disk_mb, mem_mb, ncpu);
    printf("self=%s\n", self);
    printf("\n");
    printf("[user]   userspace only — a property of the CPU, NOT of the OS.\n");
    printf("         Linux on this machine must produce the same numbers.\n");
    printf("[kernel] dominated by kernel code — this is where an OS is fast or slow.\n");
    printf("The `linux:` hints are order-of-magnitude orientation for a modern\n");
    printf("x86_64 box, not a target. For a real comparison run THIS binary on\n");
    printf("Linux on the SAME machine and diff the output.\n");
    printf("\n");

    char hb[32];
    double r;

    // Values kept for the RATIOS section.
    double cpu_chain_mops = NA, getpid_ns = NA, pipe_proc_ns = NA;
    double sleep_idle_us = NA, sleep_load_us = NA;
    double sleep_idle_max_us = NA, sleep_load_max_us = NA;
    double smp1 = NA, smpn = NA;
    double fork_copy_ratio = NA;

    // ---- CPU ----
    if (want(only, "cpu")) {
        line();
        printf("CPU\n");
        r = timed_oprate(cpu_int_chain, g_budget_ns);
        cpu_chain_mops = r / 1e6;
        row("[user]", "int latency (dependent)", cpu_chain_mops, "Mops/s", "");
        r = timed_oprate(cpu_int_tput, g_budget_ns);
        row("[user]", "int throughput (4-wide)", r * 4 / 1e6, "Mops/s", "");
        r = timed_oprate(cpu_double_chain, g_budget_ns);
        row("[user]", "float latency (dependent)", r / 1e6, "Mops/s", "");
        printf("  (if these differ from Linux on this machine, it is the P-state\n");
        printf("   governor or the hypervisor — no kernel path is being measured.)\n");
    } else {
        // The RATIOS section needs a clock reference even when CPU is skipped.
        cpu_chain_mops = timed_oprate(cpu_int_chain, g_short_ns) / 1e6;
    }

    // ---- Memory ----
    if (want(only, "mem")) {
        line();
        printf("MEMORY (working set %zu MiB)\n", mem_mb);
        g_mem_bytes = mem_mb * 1024 * 1024;
        g_mem_src = malloc(g_mem_bytes);
        g_mem_dst = malloc(g_mem_bytes);
        size_t chase_n = g_mem_bytes / sizeof(size_t);
        g_chase = malloc(chase_n * sizeof(size_t));
        if (!g_mem_src || !g_mem_dst || !g_chase) {
            printf("  (allocation failed — try a smaller MEM_MB)\n");
        } else {
            // Pre-fault both buffers: this section is meant to measure the
            // memory system, not the page-fault path (that is the VM section).
            memset(g_mem_src, 0xa5, g_mem_bytes);
            memset(g_mem_dst, 0x5a, g_mem_bytes);
            r = timed_oprate(mem_copy, g_budget_ns);
            hr_bytes(r * (double)g_mem_bytes, hb, sizeof hb);
            printf("  %-8s %-28s %12s\n", "[user]", "memcpy bandwidth", hb);
            r = timed_oprate(mem_set, g_budget_ns);
            hr_bytes(r * (double)g_mem_bytes, hb, sizeof hb);
            printf("  %-8s %-28s %12s\n", "[user]", "memset bandwidth", hb);
            build_chase(chase_n);
            r = timed_oprate(mem_chase, g_budget_ns);
            row("[user]", "random access latency", 1e9 / r, "ns", "(pre-faulted)");
        }
        free(g_mem_src); free(g_mem_dst); free(g_chase);
        g_mem_src = g_mem_dst = NULL; g_chase = NULL;
    }

    // ---- Syscall ----
    g_devnull = open("/dev/null", O_RDWR);
    g_devzero = open("/dev/zero", O_RDONLY);
    if (want(only, "syscall")) {
        line();
        printf("SYSCALL (round trip into the kernel and back)\n");
        printf("  vDSO: %s\n", vdso_presence());
        getpid_ns = timed_ns_per_op(sc_getpid, g_short_ns);
        row("[kernel]", "getpid()", getpid_ns, "ns", "linux: ~55");
        row("[kernel]", "clock_gettime(MONOTONIC)",
            timed_ns_per_op(sc_clock_gettime, g_short_ns), "ns",
            "linux: ~25 (vDSO, no trap)");
        // REALTIME takes a different path inside the vDSO (it adds the kernel's
        // wall-clock offset), and `gettimeofday`/`time` are separate entry
        // points that a glibc program calls directly. A vDSO that serves only
        // MONOTONIC would look complete in the row above and still leave every
        // timestamp in a log line going through a trap.
        row("[kernel]", "clock_gettime(REALTIME)",
            timed_ns_per_op(sc_clock_gettime_real, g_short_ns), "ns",
            "linux: ~25 (vDSO, no trap)");
        row("[kernel]", "gettimeofday()",
            timed_ns_per_op(sc_gettimeofday, g_short_ns), "ns",
            "linux: ~25 (vDSO, no trap)");
        row("[kernel]", "time()",
            timed_ns_per_op(sc_time, g_short_ns), "ns",
            "linux: ~25 (vDSO, no trap)");
        // Full signal delivery: trap in on `kill`, frame set-up on the user
        // stack, run the handler, `rt_sigreturn` back out. Two kernel entries
        // and a context save/restore -- the path every Ctrl-C, timer signal and
        // crash handler takes.
        {
            struct sigaction sa;
            memset(&sa, 0, sizeof sa);
            sa.sa_handler = bench_sig_handler;
            sigaction(SIGUSR1, &sa, NULL);
        }
        row("[kernel]", "raise(SIGUSR1)+handler",
            timed_ns_per_op(sc_signal_self, g_short_ns), "ns", "linux: ~1500");
        row("[kernel]", "sigprocmask()",
            timed_ns_per_op(sc_sigprocmask, g_short_ns), "ns", "linux: ~90");
        row("[kernel]", "sched_yield()",
            timed_ns_per_op(sc_sched_yield, g_short_ns), "ns", "linux: ~300");
        row("[kernel]", "pread(/dev/zero, 1B)",
            g_devzero >= 0 ? timed_ns_per_op(sc_read1, g_short_ns) : NA, "ns",
            "linux: ~250");
        row("[kernel]", "write(/dev/null, 1B)",
            g_devnull >= 0 ? timed_ns_per_op(sc_write1, g_short_ns) : NA, "ns",
            "linux: ~250");
        row("[kernel]", "fstat()",
            g_devnull >= 0 ? timed_ns_per_op(sc_fstat, g_short_ns) : NA, "ns",
            "linux: ~300");
        row("[kernel]", "stat(\"/dev/null\")",
            timed_ns_per_op(sc_stat_path, g_short_ns), "ns",
            "linux: ~900 (path walk)");
        row("[kernel]", "open+close(/dev/null)",
            timed_ns_per_op(sc_open_close, g_short_ns), "ns", "linux: ~1200");
        printf("  (subtract the getpid row from any other to isolate that\n");
        printf("   subsystem's cost from raw trap overhead.)\n");
    } else {
        getpid_ns = timed_ns_per_op(sc_getpid, g_short_ns / 2);
    }

    // ---- VM ----
    if (want(only, "vm")) {
        line();
        printf("VM / PAGE FAULTS\n");
        row("[kernel]", "mmap+munmap (4 KiB)",
            timed_ns_per_op(sc_mmap_munmap, g_short_ns), "ns", "linux: ~2500");
        row("[kernel]", "mprotect (4 KiB, x2)",
            timed_ns_per_op(sc_mprotect, g_short_ns), "ns", "linux: ~1800");
        row("[kernel]", "minor fault (anon touch)",
            vm_minor_fault_ns(g_short_ns), "ns", "linux: ~500");
        row("[kernel]", "COW fault (after fork)", vm_cow_fault_ns(), "ns",
            "linux: ~1500");
        {
            int cow = vm_cow_isolation_check();
            printf("  %-8s %-28s %12s", "[kernel]", "fork memory isolation",
                   cow == 0 ? "PASS" : (cow < 0 ? "n/a" : "FAIL"));
            if (cow > 0)
                printf("  <-- check %d: parent and child are NOT isolated", cow);
            printf("\n");
        }
        printf("  (every program pays these on startup and on every allocation\n");
        printf("   it touches; they never appear in a memcpy benchmark.)\n");
    }

    // ---- Scheduler ----
    if (want(only, "sched")) {
        line();
        printf("SCHEDULER / IPC   <-- what makes a system feel fast or slow\n");
        pipe_proc_ns = sched_pipe_rt_proc(g_short_ns);
        row("[kernel]", "pipe round trip (2 procs)",
            pipe_proc_ns < 0 ? NA : pipe_proc_ns / 1000.0, "us",
            "linux: ~6");
        r = sched_pipe_rt_thread(g_short_ns);
        row("[kernel]", "pipe round trip (2 thrds)", r < 0 ? NA : r / 1000.0, "us",
            "linux: ~4");
        r = sched_socketpair_rt_proc(g_short_ns);
        row("[kernel]", "socketpair round trip",
            r < 0 ? NA : r / 1000.0, "us", "linux: ~8");
        r = sched_futex_rt_ns(g_short_ns);
        row("[kernel]", "futex wake round trip",
            r < 0 ? NA : r / 1000.0, "us", "linux: ~3");
        r = sched_thread_spawn_ns(g_short_ns);
        row("[kernel]", "pthread_create + join",
            r < 0 ? NA : r / 1000.0, "us", "linux: ~15");
        row("[kernel]", "pipe bandwidth (64K writes)",
            sched_pipe_bw_mbs(g_short_ns), "MB/s", "linux: >1000");

        sleep_idle_us = sleep_overshoot_us(1000, 40, &sleep_idle_max_us);
        row("[kernel]", "sleep 1ms late, idle (mean)", sleep_idle_us, "us",
            "linux: ~60");
        row("[kernel]", "sleep 1ms late, idle (worst)", sleep_idle_max_us, "us",
            "linux: ~200");

        // The headline. Saturate every CPU with a userspace spinner that never
        // issues a syscall, then measure the same sleep and the same pipe round
        // trip. On a kernel that preempts on wake-up these barely move; on one
        // that makes a woken task wait out the running thread's timeslice they
        // jump to milliseconds — and that is what using the machine feels like.
        pid_t *hogs = calloc((size_t)ncpu, sizeof *hogs);
        int nhogs = hogs ? load_start(hogs, ncpu) : 0;
        if (nhogs > 0) {
            // Let the hogs actually get scheduled before measuring.
            struct timespec settle = {0, 50 * 1000 * 1000};
            nanosleep(&settle, NULL);
            printf("  -- now with %d CPU-bound processes competing for the CPUs --\n",
                   nhogs);
            sleep_load_us = sleep_overshoot_us(1000, 40, &sleep_load_max_us);
            row("[kernel]", "sleep 1ms late, load (mean)", sleep_load_us, "us",
                "linux: ~100");
            row("[kernel]", "sleep 1ms late, load (worst)", sleep_load_max_us,
                "us", "linux: ~500  <-- stutter");
            double loaded_pipe = sched_pipe_rt_proc(g_short_ns);
            row("[kernel]", "pipe round trip, load",
                loaded_pipe < 0 ? NA : loaded_pipe / 1000.0, "us", "linux: ~20");
            load_stop(hogs, nhogs);
        } else {
            printf("  (could not start CPU hogs — loaded latency not measured)\n");
        }
        free(hogs);
        printf("  wake-up latency under load is THE interactivity metric. A kernel\n");
        printf("  that only reschedules at timeslice expiry shows a large jump\n");
        printf("  between the idle and loaded rows; one that preempts on wake-up\n");
        printf("  (Linux does) shows almost none.\n");
    }

    // ---- SMP ----
    if (want(only, "smp")) {
        line();
        printf("SMP SCALING (%d CPUs online)\n", ncpu);
        smp1 = smp_aggregate(1, g_short_ns);
        row("[kernel]", "1 thread aggregate", smp1, "Mops/s", "");
        if (ncpu > 1) {
            smpn = smp_aggregate(ncpu, g_short_ns);
            char lbl[64];
            snprintf(lbl, sizeof lbl, "%d threads aggregate", ncpu);
            row("[kernel]", lbl, smpn, "Mops/s", "");
            if (smp1 > 0 && smpn > 0) {
                double eff = smpn / (smp1 * ncpu) * 100.0;
                row("[kernel]", "scaling efficiency", eff, "%",
                    "linux: >90 on this workload");
            }
        }
        printf("  (the work is pure userspace ALU, so anything short of linear\n");
        printf("   scaling is the kernel: placement, contention or offline CPUs.)\n");
        printf("  -- kernel SMP paths: contention, shootdowns, cross-CPU wakes --\n");
        {
            char lbl[64];
            // Syscall entry from every CPU at once. Per-cpu state done right
            // scales ~linearly; a shared hot lock on the entry path shows up
            // as efficiency collapsing.
            double s1 = smpk_rate(1, smpk_getpid_op, g_short_ns);
            double sn = ncpu > 1 ? smpk_rate(ncpu, smpk_getpid_op, g_short_ns) : NA;
            row("[kernel]", "getpid/s x1", s1 < 0 ? NA : s1 / 1e6, "Mops/s", "");
            snprintf(lbl, sizeof lbl, "getpid/s x%d", ncpu);
            row("[kernel]", lbl, sn < 0 ? NA : sn / 1e6, "Mops/s", "");
            if (s1 > 0 && sn > 0)
                row("[kernel]", "syscall scaling", sn / (s1 * ncpu) * 100.0, "%",
                    "linux: >85");

            // The TLB shootdown, isolated: same flip, idle peers vs spinning
            // peers. The difference is the cross-CPU invalidation.
            double m0 = smpk_mprotect_ns(0, g_short_ns);
            double mn = ncpu > 1 ? smpk_mprotect_ns(ncpu - 1, g_short_ns) : NA;
            row("[kernel]", "mprotect flip, peers idle", m0, "ns", "");
            snprintf(lbl, sizeof lbl, "mprotect flip, %d spinning", ncpu - 1);
            row("[kernel]", lbl, mn, "ns", "");
            if (m0 > 0 && mn > 0)
                row("[kernel]", "shootdown cost", mn / m0, "x",
                    "linux: ~2-4 (IPI + ack wait)");

            // Parallel mappers and faulters: the VM locks.
            double mm1 = smpk_rate(1, smpk_mmap_op, g_short_ns);
            double mmn = ncpu > 1 ? smpk_rate(ncpu, smpk_mmap_op, g_short_ns) : NA;
            snprintf(lbl, sizeof lbl, "mmap+touch+munmap x%d vs x1", ncpu);
            if (mm1 > 0 && mmn > 0)
                row("[kernel]", lbl, mmn / (mm1 * ncpu) * 100.0, "%",
                    "linux: ~40-70 (mmap_lock)");
            double f1 = smpk_rate(1, smpk_fault_op, g_short_ns);
            double fn = ncpu > 1 ? smpk_rate(ncpu, smpk_fault_op, g_short_ns) : NA;
            row("[kernel]", "minor faults/s x1",
                f1 < 0 ? NA : f1 * 64.0 / 1e3, "kflt/s", "");
            snprintf(lbl, sizeof lbl, "minor faults/s x%d", ncpu);
            row("[kernel]", lbl, fn < 0 ? NA : fn * 64.0 / 1e3, "kflt/s", "");
            if (f1 > 0 && fn > 0)
                row("[kernel]", "fault scaling", fn / (f1 * ncpu) * 100.0, "%",
                    "linux: >60");

            // One mutex, everyone. The x1 row is the uncontended fast path
            // (never enters the kernel); the xN row is the futex sleep/wake
            // machinery under fire. The collapse factor is what contention
            // costs on this kernel.
            double x1 = smpk_rate(1, smpk_mutex_op, g_short_ns);
            double xn = ncpu > 1 ? smpk_rate(ncpu, smpk_mutex_op, g_short_ns) : NA;
            if (x1 > 0 && xn > 0)
                row("[kernel]", "contended mutex collapse", x1 / xn, "x",
                    "linux: ~10-40 under full contention");

            // Where a wake lands: same CPU (context switch, hot cache) against
            // a neighbouring CPU (remote wake, IPI).
            if (ncpu > 1) {
                double same = smpk_pipe_rt_pinned(0, 0, ncpu, g_short_ns);
                double cross = smpk_pipe_rt_pinned(0, 1, ncpu, g_short_ns);
                row("[kernel]", "pipe RT pinned same-CPU",
                    same < 0 ? NA : same / 1000.0, "us", "");
                row("[kernel]", "pipe RT pinned cross-CPU",
                    cross < 0 ? NA : cross / 1000.0, "us", "");
                if (same > 0 && cross > 0)
                    row("[kernel]", "cross-CPU wake cost", cross / same, "x",
                        "linux: ~0.5-2");
            }

            // Everybody forks at once: the copy-on-write machinery colliding.
            double fk1 = smpk_forks_per_s(1, g_short_ns);
            double fkn = ncpu > 1 ? smpk_forks_per_s(ncpu, g_short_ns) : NA;
            row("[kernel]", "forks/s x1", fk1, "forks/s", "");
            snprintf(lbl, sizeof lbl, "forks/s x%d procs", ncpu);
            row("[kernel]", lbl, fkn, "forks/s", "");
            if (fk1 > 0 && fkn > 0)
                row("[kernel]", "fork scaling", fkn / (fk1 * ncpu) * 100.0, "%",
                    "linux: ~50-80");

            // 2xN hogs on N CPUs for a while: does everyone get a fair share?
            double fair = smpk_fairness_maxmin(2 * ncpu, g_short_ns);
            row("[kernel]", "fairness max/min (2x hogs)", fair, "x",
                "1.0 = perfectly fair; linux: <1.5");
            printf("  x1 rows are each xN row's own baseline, so every scaling\n");
            printf("  figure is hardware-independent. The shootdown row is the\n");
            printf("  one that punishes a slow IPI/ack path; the fairness row\n");
            printf("  catches a scheduler that posts great aggregates by\n");
            printf("  starving somebody.\n");
        }
    }

    // ---- Disk ----
    if (want(only, "disk")) {
        line();
        printf("DISK (in %s, %zu MiB requested)\n", dir, disk_mb);
        size_t dbytes = disk_mb * 1024 * 1024;
        int meta_max = 4000;
        // Cap the disk working set to a fraction of the FREE space. A small or
        // nearly-full filesystem (notably the in-RAM SFS root that `make qemu`
        // boots) must not be filled: some filesystems panic on ENOSPC instead of
        // failing the write, which would crash the whole machine mid-benchmark.
        {
            struct statvfs vfs;
            if (statvfs(dir, &vfs) == 0 && vfs.f_bavail > 0) {
                unsigned long bs = vfs.f_frsize ? vfs.f_frsize : vfs.f_bsize;
                unsigned long long freeb = (unsigned long long)vfs.f_bavail * bs;
                unsigned long long usable = freeb / 3;
                if ((unsigned long long)dbytes > usable) dbytes = (size_t)usable;
                unsigned long long mm = usable / (8 * 1024); // ~8 KiB/small file
                if (mm < (unsigned long long)meta_max) meta_max = (int)mm;
                printf("  free=%llu MiB -> file %zu MiB, up to %d meta files\n",
                       freeb / (1024 * 1024), dbytes / (1024 * 1024), meta_max);
            } else {
                if (dbytes > 4u * 1024 * 1024) dbytes = 4u * 1024 * 1024;
                if (meta_max > 500) meta_max = 500;
                printf("  (statvfs unavailable — capping to %zu MiB / %d files)\n",
                       dbytes / (1024 * 1024), meta_max);
            }
        }

        const size_t chunk = 256 * 1024;
        unsigned char *io = malloc(chunk);
        if (!io) {
            printf("  (io buffer alloc failed)\n");
        } else {
            memset(io, 0x5a, chunk);
            char fpath[512];
            snprintf(fpath, sizeof fpath, "%s/eclipse-bench.dat", dir);

            if (dbytes >= 1u * 1024 * 1024) {
                double w = disk_seq_write(fpath, dbytes, chunk, io);
                if (w < 0) printf("  %-8s %-28s %12s\n", "[kernel]", "seq write (+fsync)", "FAILED");
                else { hr_bytes(w, hb, sizeof hb);
                       printf("  %-8s %-28s %12s\n", "[kernel]", "seq write (+fsync)", hb); }

                double rd = disk_seq_read(fpath, chunk, io);
                if (rd < 0) printf("  %-8s %-28s %12s\n", "[kernel]", "seq read", "FAILED");
                else { hr_bytes(rd, hb, sizeof hb);
                       printf("  %-8s %-28s %12s\n", "[kernel]", "seq read", hb); }

                double avg_us = 0;
                double iops = disk_rand_read(fpath, dbytes, g_budget_ns, &avg_us);
                row("[kernel]", "rand 4K read", iops, "IOPS", "");
                if (iops > 0)
                    row("[kernel]", "rand 4K read latency", avg_us, "us", "");

                row("[kernel]", "fsync latency (best)", disk_fsync_ms(fpath, io),
                    "ms", "");
                unlink(fpath);
            } else {
                printf("  (too little free space for the streaming tests — point\n");
                printf("   DIR at a real disk/partition with more room)\n");
            }

            if (meta_max >= 20) {
                double cps, sps, ups;
                disk_metadata(dir, g_budget_ns, meta_max, &cps, &sps, &ups);
                row("[kernel]", "meta create small files", cps, "files/s", "");
                row("[kernel]", "meta stat", sps, "stats/s", "");
                row("[kernel]", "meta unlink", ups, "unlinks/s", "");
            } else {
                printf("  meta ops: skipped (low free space)\n");
            }
            free(io);
        }
    }

    // ---- Process ----
    if (want(only, "proc")) {
        line();
        printf("PROCESS CREATION\n");
        double fr = proc_fork_ns(g_short_ns);
        row("[kernel]", "fork + exit", fr < 0 ? NA : fr / 1000.0, "us",
            "linux: ~70");
        // Copy-on-write check. The slope between the two sizes is the cost the
        // kernel charges per MiB of the parent's resident set, every fork.
        double f1 = proc_fork_resident_ns(1, g_short_ns);
        double f16 = proc_fork_resident_ns(16, g_short_ns);
        row("[kernel]", "fork + exit, 1 MiB resident",
            f1 < 0 ? NA : f1 / 1000.0, "us", "");
        row("[kernel]", "fork + exit, 16 MiB resident",
            f16 < 0 ? NA : f16 / 1000.0, "us", "");
        if (f1 > 0 && f16 > 0) {
            double per_mib = (f16 - f1) / 15.0 / 1000.0;
            // A negative slope is not a failed measurement — it is the answer.
            // Copy-on-write makes fork cost independent of the resident set, so
            // the two sizes land within noise of each other and the difference
            // can come out either side of zero on a loaded host. Clamping to
            // zero reports "flat" instead of the "n/a" that a negative value
            // used to produce, which read like the probe had broken.
            if (per_mib < 0)
                per_mib = 0;
            row("[kernel]", "fork cost per MiB resident", per_mib, "us/MiB",
                per_mib == 0 ? "flat within noise" : "");
            // Absolute microseconds per MiB say nothing on their own: a slow
            // machine is slow at everything. What settles it is how that cost
            // compares to what copying a MiB *costs on this very machine*. An
            // eager fork must pay at least one memcpy per resident MiB, so the
            // ratio lands near (or above) 1. A copy-on-write fork only touches
            // page tables — measured at ~0.3 on Linux, where the residual is
            // the page-table copy plus the child's teardown of the mappings on
            // exit, both of which every kernel pays.
            uint64_t mb = 1024 * 1024;
            unsigned char *a = malloc(mb), *b = malloc(mb);
            if (a && b) {
                memset(a, 0x5a, mb);
                memset(b, 0, mb);
                uint64_t c0 = now_ns();
                int reps = 16;
                for (int i = 0; i < reps; i++)
                    memcpy(b, a, mb);
                double memcpy_us_per_mib =
                    (double)(now_ns() - c0) / (double)reps / 1000.0;
                g_sink += b[0];
                fork_copy_ratio = per_mib / memcpy_us_per_mib;
                row("[kernel]", "  vs memcpy 1 MiB here", memcpy_us_per_mib,
                    "us/MiB", "");
                row("", "fork copy ratio", fork_copy_ratio, "x",
                    "COW ~0.3, eager copy >=1");
            }
            free(a); free(b);
            // The other axis: cost per mapping, with the resident set held
            // fixed. Copy-on-write trades a per-page cost for a per-mapping one,
            // and if the kernel shoots down every other CPU's TLB once per
            // mapping that trade can lose badly for an ordinary process, which
            // has many small mappings and few large ones.
            double m8 = proc_fork_mappings_ns(8, g_short_ns);
            double m256 = proc_fork_mappings_ns(256, g_short_ns);
            row("[kernel]", "fork + exit, 8 mappings",
                m8 < 0 ? NA : m8 / 1000.0, "us", "");
            row("[kernel]", "fork + exit, 256 mappings",
                m256 < 0 ? NA : m256 / 1000.0, "us", "");
            if (m8 > 0 && m256 > 0) {
                double per_map = (m256 - m8) / 248.0 / 1000.0;
                if (per_map < 0)
                    per_map = 0;
                row("[kernel]", "fork cost per mapping", per_map, "us/mapping",
                    per_map == 0 ? "flat within noise" : "");
            }
            // The same probe with every other CPU busy, which is a different
            // measurement and not merely a noisier one.
            //
            // A fork write-protects each mapping and must then invalidate the
            // other CPUs' TLBs. A kernel is free to skip the wait for a CPU that
            // is *halted* — it holds no live entry and will flush before it runs
            // anything — so on an otherwise idle machine the shootdown costs
            // almost nothing and this whole class of cost is invisible. Put the
            // other CPUs to work and each shootdown becomes a real IPI round
            // trip with an acknowledgement to wait for.
            //
            // That is also the honest case: a shell forks while the machine is
            // doing something, not while it sits idle.
            int nload = ncpu > 1 ? ncpu - 1 : 0;
            pid_t *fl = nload > 0 ? calloc((size_t)nload, sizeof *fl) : NULL;
            int nfl = fl ? load_start(fl, nload) : 0;
            if (nfl > 0) {
                // Let the hogs actually get scheduled before measuring.
                struct timespec settle = {0, 120 * 1000 * 1000};
                nanosleep(&settle, NULL);
                double lm8 = proc_fork_mappings_ns(8, g_short_ns);
                double lm256 = proc_fork_mappings_ns(256, g_short_ns);
                load_stop(fl, nfl);
                printf("  -- now with %d CPU-bound processes competing --\n", nfl);
                row("[kernel]", "fork+exit, 8 maps, load",
                    lm8 < 0 ? NA : lm8 / 1000.0, "us", "");
                row("[kernel]", "fork+exit, 256 maps, load",
                    lm256 < 0 ? NA : lm256 / 1000.0, "us", "");
                if (lm8 > 0 && lm256 > 0) {
                    double per_map_load = (lm256 - lm8) / 248.0 / 1000.0;
                    if (per_map_load < 0)
                        per_map_load = 0;
                    row("[kernel]", "fork cost per mapping, load", per_map_load,
                        "us/mapping", per_map_load == 0 ? "flat within noise" : "");
                }
            }
            free(fl);
            printf("  Per-mapping cost is the blind spot of the per-MiB row above.\n");
            printf("  A copy-on-write fork stops paying per page and starts paying\n");
            printf("  per mapping, so a process with a hundred small mappings can\n");
            printf("  fork far more slowly than one holding the same bytes in a\n");
            printf("  single large one -- which no per-MiB number can express.\n");
            printf("  Compare the idle and loaded rows to separate the two things\n");
            printf("  that cost per mapping: the bookkeeping (same in both) and the\n");
            printf("  cross-CPU TLB shootdown (only paid when a peer is awake).\n");
            printf("  This is the copy-on-write test, and it matters more than any\n");
            printf("  single fork number. A copy-on-write fork shares the parent's\n");
            printf("  frames and write-protects them, so a process holding 100 MiB\n");
            printf("  forks about as cheaply as one holding nothing. A fork that\n");
            printf("  copies every resident frame up front is O(resident): the same\n");
            printf("  shell running the same command pays a full memcpy of the\n");
            printf("  process every single time it forks.\n");
            printf("  NOTE: when fork copies eagerly, the `COW fault (after fork)`\n");
            printf("  row in the VM section is meaningless — the child's pages are\n");
            printf("  already private, so it times plain stores and reports an\n");
            printf("  implausibly good number.\n");
        }
        double fe = proc_fork_exec_ns(g_short_ns, self);
        row("[kernel]", "fork + exec(self, static)", fe < 0 ? NA : fe / 1000.0,
            "us", "linux: ~350");
        const char *sh = find_shell();
        if (sh) {
            double sp = proc_spawn_shell_ns(g_short_ns, sh);
            char lbl[64];
            snprintf(lbl, sizeof lbl, "fork + exec(%s -c :)", sh);
            row("[kernel]", lbl, sp < 0 ? NA : sp / 1000.0, "us", "linux: ~1200");
        } else {
            printf("  (no shell found — the real 'launch a command' cost was\n");
            printf("   not measured; it is the one a script actually pays)\n");
        }
    }

    // ---- Ratios ----
    // These are the numbers to quote when someone says "but we are in a VM".
    // Each is a kernel cost divided by something measured on the *same* machine
    // in the *same* run, so hardware speed cancels out.
    line();
    printf("RATIOS (hardware-independent — these survive running in a VM)\n");
    if (cpu_chain_mops > 0 && getpid_ns > 0) {
        // How many dependent integer ops this CPU could have retired in the time
        // one minimal syscall takes.
        double ops = getpid_ns * cpu_chain_mops / 1000.0;
        row("", "syscall cost in CPU ops", ops, "ops", "linux: ~150-250");
    }
    if (getpid_ns > 0 && pipe_proc_ns > 0)
        row("", "context switch / syscall", pipe_proc_ns / getpid_ns, "x",
            "linux: ~100");
    if (sleep_idle_us > 0 && sleep_load_us > 0)
        row("", "wake late loaded/idle (mean)", sleep_load_us / sleep_idle_us,
            "x", "linux: ~1-3");
    if (sleep_idle_max_us > 0 && sleep_load_max_us > 0)
        row("", "wake late loaded/idle (worst)",
            sleep_load_max_us / sleep_idle_max_us, "x",
            "linux: ~1-5  <-- interactivity");
    if (smp1 > 0 && smpn > 0 && ncpu > 1)
        row("", "SMP efficiency", smpn / (smp1 * ncpu) * 100.0, "%",
            "linux: >90");
    if (fork_copy_ratio > 0)
        row("", "fork copy ratio", fork_copy_ratio, "x",
            "COW ~0.3, eager copy >=1");
    printf("\n");
    printf("  `wake late loaded/idle` is the headline. A value near 1 means a\n");
    printf("  woken task gets a CPU straight away even when the machine is busy.\n");
    printf("  A large value means it waits for someone else's timeslice to run\n");
    printf("  out — the system will feel sluggish no matter how good the [user]\n");
    printf("  numbers above look. The (worst) row is the stutter you actually\n");
    printf("  notice; a mean can hide one 20 ms stall in forty prompt wakes.\n");

    if (g_devnull >= 0) close(g_devnull);
    if (g_devzero >= 0) close(g_devzero);

    line();
    printf("done. (g_sink=%llu)\n", (unsigned long long)g_sink);
    return 0;
}
