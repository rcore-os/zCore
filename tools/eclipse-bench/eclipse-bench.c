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
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
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

// Per-microbench wall-clock budgets. `--quick` scales them down.
static uint64_t g_budget_ns = 400000000ull;  // 0.4 s — the classic sections
static uint64_t g_short_ns = 200000000ull;   // 0.2 s — the many small probes

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

// Run `fn()` repeatedly for `budget_ns` and return the mean nanoseconds per
// call. `fn` returns 0 on success and non-zero to abort the measurement (the
// operation is unsupported on this kernel), in which case NA is returned.
static double timed_ns_per_op(int (*fn)(void), uint64_t budget_ns) {
    // Warm up once so a lazily-initialised path (first mmap of an arena, first
    // open of a device) is not charged to the measurement.
    if (fn() != 0)
        return NA;
    uint64_t t0 = now_ns(), elapsed = 0, ops = 0;
    while (elapsed < budget_ns) {
        for (int k = 0; k < 64; k++) {
            if (fn() != 0)
                return NA;
            ops++;
        }
        elapsed = now_ns() - t0;
    }
    return (double)elapsed / (double)ops;
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
    while (elapsed < budget_ns) {
        for (int k = 0; k < 32; k++) {
            if (write(pp->a[1], &c, 1) != 1)
                return NA;
            if (read(pp->b[0], &c, 1) != 1)
                return NA;
            ops++;
        }
        elapsed = now_ns() - t0;
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

struct spin_arg {
    uint64_t deadline_ns;
    uint64_t iters;
};

static void *spin_thread(void *arg) {
    struct spin_arg *a = arg;
    uint64_t x = 0x9e3779b97f4a7c15ull, n = 0;
    while (now_ns() < a->deadline_ns) {
        for (int k = 0; k < 4096; k++)
            x = x * 6364136223846793005ull + 1442695040888963407ull;
        n += 4096;
    }
    g_sink += x;
    a->iters = n;
    return NULL;
}

// Aggregate Mops/s across `n` threads over `budget_ns`. NA if threads fail.
static double smp_aggregate(int n, uint64_t budget_ns) {
    if (n < 1)
        return NA;
    pthread_t *th = calloc((size_t)n, sizeof *th);
    struct spin_arg *args = calloc((size_t)n, sizeof *args);
    if (!th || !args) { free(th); free(args); return NA; }
    uint64_t t0 = now_ns();
    uint64_t deadline = t0 + budget_ns;
    int started = 0;
    for (int i = 0; i < n; i++) {
        args[i].deadline_ns = deadline;
        args[i].iters = 0;
        if (pthread_create(&th[i], NULL, spin_thread, &args[i]) != 0)
            break;
        started++;
    }
    if (started == 0) { free(th); free(args); return NA; }
    uint64_t total = 0;
    for (int i = 0; i < started; i++) {
        pthread_join(th[i], NULL);
        total += args[i].iters;
    }
    uint64_t dt = now_ns() - t0;
    free(th);
    free(args);
    if (started != n || dt == 0)
        return NA; // could not run the requested width — do not report a lie
    return (double)total * 1e9 / (double)dt / 1e6;
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
    while (elapsed < budget_ns) {
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

// fork + immediate child _exit, parent waits. Returns ns per fork.
static double proc_fork_ns(uint64_t budget_ns) {
    uint64_t ops = 0, t0 = now_ns(), elapsed = 0;
    while (elapsed < budget_ns) {
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
    while (elapsed < budget_ns) {
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
    while (elapsed < budget_ns) {
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

    const char *only = NULL;
    int argi = 1;
    while (argi < argc && argv[argi][0] == '-' && argv[argi][1] == '-') {
        if (strcmp(argv[argi], "--only") == 0 && argi + 1 < argc) {
            only = argv[++argi];
        } else if (strcmp(argv[argi], "--quick") == 0) {
            g_budget_ns = 150000000ull;
            g_short_ns = 60000000ull;
        } else {
            fprintf(stderr, "usage: %s [--only SECTION] [--quick] [DIR] [DISK_MB] [MEM_MB]\n",
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
        getpid_ns = timed_ns_per_op(sc_getpid, g_short_ns);
        row("[kernel]", "getpid()", getpid_ns, "ns", "linux: ~55");
        row("[kernel]", "clock_gettime(MONOTONIC)",
            timed_ns_per_op(sc_clock_gettime, g_short_ns), "ns",
            "linux: ~25 (vDSO, no trap)");
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
