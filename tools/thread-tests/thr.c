/*
 * Freestanding multithreading test for Eclipse OS (zCore) libos mode.
 *
 * Reproduces musl 1.2.x pthread_create kernel interaction without libc:
 *   clone(flags=0x7d0f00, stack, ptid=&tid, tls, ctid=&list_lock)
 * then exercises FUTEX_WAIT / FUTEX_WAKE between the two threads and
 * verifies CHILD_CLEARTID wake on thread exit (what pthread_join needs).
 *
 * All syscalls go through the `rcore_syscall_entry` trampoline pointer,
 * which the zCore ELF loader patches to its in-process syscall entry.
 *
 * Build: gcc -static -nostdlib -fno-stack-protector -fno-builtin -O1 -o thr thr.c
 */

typedef unsigned long u64;
typedef long i64;

/* Patched by the zCore loader with the kernel syscall entry address.
 * Must live in .data (initialized) so the write lands inside the ELF VMO. */
__attribute__((used, section(".rodata.rcore"))) u64 rcore_syscall_entry = 0xdeadbeef;

/* libos runs user code in-process: syscalls are `call` through the patched
 * trampoline. Bare metal uses the real `syscall` instruction. */
#ifdef VIA_CALL
#define SYSCALL_INSN "call *rcore_syscall_entry(%%rip)"
#else
#define SYSCALL_INSN "syscall"
#endif

#define SYS_write 1
#define SYS_mmap 9
#define SYS_clone 56
#define SYS_exit 60
#define SYS_exit_group 231
#define SYS_futex 202
#define SYS_sched_yield 24
#define SYS_nanosleep 35

#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#define FUTEX_PRIVATE 128

struct timespec {
    i64 tv_sec;
    i64 tv_nsec;
};

static i64 sc6(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e, i64 f)
{
    register i64 r10 __asm__("r10") = d;
    register i64 r8 __asm__("r8") = e;
    register i64 r9 __asm__("r9") = f;
    i64 ret;
    __asm__ volatile(SYSCALL_INSN
                     : "=a"(ret)
                     : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8),
                       "r"(r9)
                     : "rcx", "r11", "memory");
    return ret;
}
#define sc3(n, a, b, c) sc6(n, (i64)(a), (i64)(b), (i64)(c), 0, 0, 0)

static void print(const char *s)
{
    i64 len = 0;
    while (s[len])
        len++;
    sc3(SYS_write, 1, s, len);
}

static void print_num(const char *pfx, i64 v)
{
    char buf[32];
    int i = 31, neg = v < 0;
    u64 x = neg ? (u64)-v : (u64)v;
    buf[i--] = 0;
    do {
        buf[i--] = '0' + (x % 10);
        x /= 10;
    } while (x);
    if (neg)
        buf[i--] = '-';
    print(pfx);
    print(&buf[i + 1]);
    print("\n");
}

static void sleep_ms(i64 ms)
{
    struct timespec ts = {ms / 1000, (ms % 1000) * 1000000};
    sc3(SYS_nanosleep, &ts, 0, 0);
}

/* ---- test state ---- */
static int tid_slot;              /* ptid                                  */
static int list_lock = 0x1111;    /* ctid; kernel must not touch at create */
static volatile int progress;     /* child stage                           */
static volatile int gate;         /* parent->child handshake futex         */
static u64 child_tls[64];         /* fake TLS block (fs base for child)    */

static int child_fn(void *arg)
{
    progress = 1;
    u64 self;
    __asm__ volatile("mov %%fs:0, %0" : "=r"(self));
    if (self == (u64)child_tls)
        progress = 2;
    sc6(SYS_futex, (i64)&progress, FUTEX_WAKE | FUTEX_PRIVATE, 1, 0, 0, 0);

    /* wait for parent to open the gate (tests futex wait+wake both ways) */
    while (!gate)
        sc6(SYS_futex, (i64)&gate, FUTEX_WAIT | FUTEX_PRIVATE, 0, 0, 0, 0);

    progress = 3;
    sc6(SYS_futex, (i64)&progress, FUTEX_WAKE | FUTEX_PRIVATE, 1, 0, 0, 0);
    sc3(SYS_exit, 0, 0, 0); /* kernel must clear+wake list_lock */
    return 0;
}

/* musl __clone for x86_64 with `call` trampoline instead of `syscall`. */
static i64 musl_clone(int (*fn)(void *), void *stack_top, i64 flags, void *arg,
                      int *ptid, void *tls, int *ctid)
{
    i64 ret;
    u64 sp = ((u64)stack_top & -16UL) - 8;
    *(void **)sp = arg;
    register i64 r8 __asm__("r8") = (i64)tls;
    register i64 r9 __asm__("r9") = (i64)fn;
    register i64 r10 __asm__("r10") = (i64)ctid;
    __asm__ volatile(SYSCALL_INSN "\n\t"
                     "test %%eax,%%eax\n\t"
                     "jnz 1f\n\t"
                     /* child */
                     "xor %%ebp,%%ebp\n\t"
                     "pop %%rdi\n\t"
                     "call *%%r9\n\t"
                     "mov %%eax,%%edi\n\t"
                     "mov $60,%%eax\n\t"
                     SYSCALL_INSN "\n\t"
                     "1:"
                     : "=a"(ret)
                     : "0"((i64)SYS_clone), "D"(flags), "S"(sp), "d"(ptid),
                       "r"(r10), "r"(r8), "r"(r9)
                     : "rcx", "r11", "memory");
    return ret;
}

__attribute__((used)) static void run_main(void);

/* Process entry: realign rsp (kernel hands it 16-aligned; C functions
 * expect entry-via-call alignment, rsp ≡ 8 mod 16). */
__attribute__((naked)) void _start(void)
{
    __asm__ volatile("and $-16, %rsp\n\t"
                     "xor %ebp, %ebp\n\t"
                     "call run_main\n\t"
                     "hlt");
}

__attribute__((used)) static void run_main(void)
{
    print("thr: start\n");
    child_tls[0] = (u64)child_tls;

    i64 stk = sc6(SYS_mmap, 0, 256 * 1024, 3 /*RW*/, 0x22 /*PRIV|ANON*/, -1,
                  0);
    if (stk < 0) {
        print_num("FAIL mmap: ", stk);
        sc3(SYS_exit_group, 1, 0, 0);
    }

    i64 flags = 0x7d0f00;
    i64 r = musl_clone(child_fn, (void *)(stk + 256 * 1024), flags, 0,
                       &tid_slot, child_tls, &list_lock);
    print_num("clone ret: ", r);
    if (r <= 0) {
        print("FAIL: clone\n");
        sc3(SYS_exit_group, 1, 0, 0);
    }
    if (tid_slot != (int)r)
        print("WARN: PARENT_SETTID not honored\n");
    if (list_lock != 0x1111)
        print("BUG: kernel wrote ctid at creation (no CHILD_SETTID)\n");

    /* wait for child to run */
    for (int i = 0; i < 50 && progress < 2; i++) {
        struct timespec ts = {0, 100 * 1000 * 1000};
        sc6(SYS_futex, (i64)&progress, FUTEX_WAIT | FUTEX_PRIVATE, progress,
            (i64)&ts, 0, 0);
    }
    print_num("child progress: ", progress);
    if (progress < 1) {
        print("FAIL: child never ran\n");
        sc3(SYS_exit_group, 1, 0, 0);
    }
    if (progress < 2)
        print("WARN: child TLS (fs base) wrong\n");

    /* set expected lock value BEFORE letting the child exit, so the
     * CHILD_CLEARTID clear/wake cannot race with us */
    list_lock = (int)r;
    gate = 1;
    sc6(SYS_futex, (i64)&gate, FUTEX_WAKE | FUTEX_PRIVATE, 1, 0, 0, 0);

    for (int i = 0; i < 50 && list_lock != 0; i++) {
        struct timespec ts = {0, 100 * 1000 * 1000};
        sc6(SYS_futex, (i64)&list_lock, FUTEX_WAIT, list_lock, (i64)&ts, 0,
            0);
    }
    if (list_lock != 0) {
        print("FAIL: CHILD_CLEARTID wake missing (pthread_join would hang)\n");
        sc3(SYS_exit_group, 1, 0, 0);
    }
    print("CHILD_CLEARTID ok\n");
    if (progress == 3)
        print("thr: PASS\n");
    else
        print_num("thr: partial, progress=", progress);
    sc3(SYS_exit_group, 0, 0, 0);
    __builtin_unreachable();
}
