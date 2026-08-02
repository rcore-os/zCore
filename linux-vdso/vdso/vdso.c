// Eclipse's Linux-compatible vDSO: the userspace half of `clock_gettime`.
//
// Reading the clock is one of the most frequently issued operations any real
// program performs — every log line, every timeout, every animation frame — and
// on Eclipse it cost a full trap into the kernel: 8597 ns measured against
// Linux's 199 ns under identical emulation, a 43x gap and by a wide margin the
// largest one left. That gap is not closable by making the trap cheaper.
// Eclipse's trap is already about three times cheaper than Linux's; Linux simply
// does not take one. It maps a tiny shared library into every process and
// answers the call in userspace off the TSC.
//
// So does this. The kernel publishes the TSC scaling factor and the wall-clock
// offset into `_vdso_data` (which lives inside this image, so the kernel writes
// it once through the shared VM object and every process sees it), and the code
// below converts a `rdtsc` into a timespec without leaving ring 3.
//
// ## Contract with the C library
//
// musl finds this through `AT_SYSINFO_EHDR` in the aux vector and
// `__vdsosym("LINUX_2.6", "__vdso_clock_gettime")` (musl/src/internal/vdso.c).
// Two details of that lookup shape this file:
//
//  * It skips version checking entirely when the image has no `DT_VERDEF`
//    (`if (!verdef) versym = 0;`), so no symbol-version script is needed
//    despite the "LINUX_2.6" it asks for.
//  * It needs `DT_SYMTAB`, `DT_STRTAB` and one of `DT_HASH` / `DT_GNU_HASH`.
//
// And on the return path, musl treats any error except `-EINVAL` as "the vDSO
// could not answer" and falls through to the real syscall:
//
//      r = f(clk, ts);
//      if (!r) return r;
//      if (r == -EINVAL) return __syscall_ret(r);
//      /* Fall through on errors other than EINVAL. */
//
// That is what makes this safe to ship. Every path this file cannot serve —
// the kernel having disabled it, a clock id it does not implement — returns
// -ENOSYS and the caller transparently gets the kernel's answer. The worst
// outcome of a bug here is no speedup; it is never a wrong time.
//
// ## Position independence without a dynamic linker
//
// Nothing relocates this image after the kernel maps it, so it must contain no
// dynamic relocations at all. That is why everything here is `hidden` except
// the three entry points: hidden symbols are reached RIP-relative, while a
// default-visibility object like `_vdso_data` would be routed through a GOT
// slot that nobody would ever fill in. The build script rejects the image if
// any dynamic relocation survives, so this cannot regress quietly.
//
// Built freestanding: no libc, no relocations against anything external, and
// nothing that could call into libgcc (the 64x64->128 multiply and the
// divisions by 1e9 all compile to inline instructions on x86_64).

#define VDSO_EXPORT __attribute__((visibility("default")))

#include <stdint.h>

// Keep in sync with `linux_vdso::VdsoData` on the kernel side.
struct vdso_data {
    // Non-zero when the fields below are valid and usable. The kernel clears
    // it when the TSC is not a usable time source (not invariant, or its
    // frequency was never calibrated), and this file then declines every
    // request so the caller falls back to the syscall.
    volatile uint32_t enabled;
    volatile uint32_t _pad;
    // Monotonic nanoseconds = (rdtsc() * tsc_mult) >> 32. Same fixed-point
    // multiplier the kernel's own `timer_now` uses.
    volatile uint64_t tsc_mult;
    // Added to monotonic nanoseconds to obtain CLOCK_REALTIME.
    volatile uint64_t wall_off_ns;
};

// No seqlock. Both payload fields are naturally-aligned 64-bit quantities, and
// on x86_64 an aligned 64-bit load cannot tear, so each is individually
// consistent. They are also independent of one another — reading a fresh
// `wall_off_ns` alongside a stale `tsc_mult` (or the reverse) yields a time that
// is correct for the instant it is read, which is all a clock owes its caller.
// The kernel writes them at boot and on `settimeofday`, i.e. essentially never.
__attribute__((section(".vdso_data"), aligned(64))) struct vdso_data _vdso_data;

#define VDSO_CLOCK_REALTIME 0
#define VDSO_CLOCK_MONOTONIC 1
#define VDSO_CLOCK_MONOTONIC_RAW 4
#define VDSO_CLOCK_REALTIME_COARSE 5
#define VDSO_CLOCK_MONOTONIC_COARSE 6

#define VDSO_ENOSYS (-38)

struct vdso_timespec {
    long tv_sec;
    long tv_nsec;
};

struct vdso_timeval {
    long tv_sec;
    long tv_usec;
};

struct vdso_timezone {
    int tz_minuteswest;
    int tz_dsttime;
};

static inline uint64_t vdso_rdtsc(void) {
    uint32_t lo, hi;
    __asm__ __volatile__("rdtsc" : "=a"(lo), "=d"(hi));
    return ((uint64_t)hi << 32) | (uint64_t)lo;
}

// ns = (tsc * mult) >> 32, in 128 bits — exactly what the kernel's
// `tsc_to_ns` does. The 64x64->128 multiply is a single `mulq`; going through
// 64-bit arithmetic instead would overflow after about 71 days of uptime, which
// is the bug that shape was chosen to avoid in the first place.
static inline uint64_t vdso_tsc_to_ns(uint64_t tsc, uint64_t mult) {
    return (uint64_t)(((unsigned __int128)tsc * (unsigned __int128)mult) >> 32);
}

// Returns 0 and fills `ns`, or non-zero if this clock cannot be served here.
static inline int vdso_now_ns(int clk, uint64_t *ns) {
    struct vdso_data *d = &_vdso_data;
    if (!d->enabled)
        return VDSO_ENOSYS;
    uint64_t mono = vdso_tsc_to_ns(vdso_rdtsc(), d->tsc_mult);
    switch (clk) {
    case VDSO_CLOCK_MONOTONIC:
    case VDSO_CLOCK_MONOTONIC_RAW:
    case VDSO_CLOCK_MONOTONIC_COARSE:
        *ns = mono;
        return 0;
    case VDSO_CLOCK_REALTIME:
    case VDSO_CLOCK_REALTIME_COARSE:
        *ns = mono + d->wall_off_ns;
        return 0;
    default:
        // Deliberately not -EINVAL: musl returns -EINVAL straight to the
        // caller as an error, so claiming a clock is invalid here would break
        // every clock the kernel supports and this file does not (per-process
        // CPU-time clocks, for instance). -ENOSYS means "ask the kernel".
        return VDSO_ENOSYS;
    }
}

VDSO_EXPORT int __vdso_clock_gettime(int clk, struct vdso_timespec *ts) {
    uint64_t ns;
    int err = vdso_now_ns(clk, &ns);
    if (err)
        return err;
    ts->tv_sec = (long)(ns / 1000000000ull);
    ts->tv_nsec = (long)(ns % 1000000000ull);
    return 0;
}

// glibc-linked programs call this one directly. musl routes `gettimeofday`
// through `clock_gettime` and so is already covered, but the symbol costs a
// dozen instructions and makes the image useful to both.
VDSO_EXPORT int __vdso_gettimeofday(struct vdso_timeval *tv, struct vdso_timezone *tz) {
    if (tv) {
        uint64_t ns;
        int err = vdso_now_ns(VDSO_CLOCK_REALTIME, &ns);
        if (err)
            return err;
        tv->tv_sec = (long)(ns / 1000000000ull);
        tv->tv_usec = (long)((ns % 1000000000ull) / 1000ull);
    }
    if (tz) {
        // Every kernel that still answers this returns UTC with no DST, and
        // callers that pass a non-NULL `tz` universally ignore the contents.
        tz->tz_minuteswest = 0;
        tz->tz_dsttime = 0;
    }
    return 0;
}

// `time(2)` — whole seconds of CLOCK_REALTIME. Returns the value, and on
// failure returns a negative errno, which the caller checks the same way.
VDSO_EXPORT long __vdso_time(long *t) {
    uint64_t ns;
    int err = vdso_now_ns(VDSO_CLOCK_REALTIME, &ns);
    if (err)
        return err;
    long secs = (long)(ns / 1000000000ull);
    if (t)
        *t = secs;
    return secs;
}
