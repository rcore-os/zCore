/*
 * Futex wait/wake stress test for Eclipse OS (zCore).
 *
 * Two threads ping-pong through futex words many times. A correct futex
 * implementation never loses a wake: FUTEX_WAIT must atomically re-check the
 * value against the expected one with respect to FUTEX_WAKE. If the kernel
 * checks the value outside the wait-queue lock, a wake can slip between the
 * check and the enqueue and the waiter sleeps until its timeout: that shows
 * up here as "lost wakeup" events.
 *
 * Build (libos):      gcc -DVIA_CALL -static-pie -nostdlib -fPIC -O1 -o thr2-libos thr2.c
 * Build (bare metal): gcc -static -no-pie -nostdlib -O1 -o thr2-metal thr2.c
 */

typedef unsigned long u64;
typedef long i64;

__attribute__((used, section(".rodata.rcore"))) u64 rcore_syscall_entry = 0xdeadbeef;

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

#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#define FUTEX_PRIVATE 128

#define ITERS 20000
#define WAIT_TIMEOUT_NS (300 * 1000 * 1000) /* 300ms: long enough to prove a lost wake */

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

static int ping, pong;
static volatile int lost_wakeups;
static volatile int done;
static u64 child_tls[64];

/* Wait until *addr >= want; count timeouts where the value had in fact
 * already advanced (i.e. the wake was lost). */
static void wait_for(int *addr, int want)
{
    for (;;) {
        int cur = __atomic_load_n(addr, __ATOMIC_SEQ_CST);
        if (cur >= want)
            return;
        struct timespec ts = {0, WAIT_TIMEOUT_NS};
        i64 r = sc6(SYS_futex, (i64)addr, FUTEX_WAIT | FUTEX_PRIVATE, cur,
                    (i64)&ts, 0, 0);
        if (r == -110 /*ETIMEDOUT*/ &&
            __atomic_load_n(addr, __ATOMIC_SEQ_CST) >= want) {
            /* value changed and we were woken only by the timeout */
            __atomic_fetch_add((int *)&lost_wakeups, 1, __ATOMIC_SEQ_CST);
            return;
        }
    }
}

static void post(int *addr, int val)
{
    __atomic_store_n(addr, val, __ATOMIC_SEQ_CST);
    sc6(SYS_futex, (i64)addr, FUTEX_WAKE | FUTEX_PRIVATE, 1, 0, 0, 0);
}

static int child_fn(void *arg)
{
    for (int i = 1; i <= ITERS; i++) {
        wait_for(&ping, i);
        post(&pong, i);
    }
    __atomic_store_n((int *)&done, 1, __ATOMIC_SEQ_CST);
    sc3(SYS_exit, 0, 0, 0);
    return 0;
}

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
    static int tid_slot, list_lock;
    print("thr2: futex stress start\n");
    child_tls[0] = (u64)child_tls;

    i64 stk = sc6(SYS_mmap, 0, 256 * 1024, 3, 0x22, -1, 0);
    if (stk < 0) {
        print("FAIL: mmap\n");
        sc3(SYS_exit_group, 1, 0, 0);
    }
    i64 r = musl_clone(child_fn, (void *)(stk + 256 * 1024), 0x7d0f00, 0,
                       &tid_slot, child_tls, &list_lock);
    if (r <= 0) {
        print("FAIL: clone\n");
        sc3(SYS_exit_group, 1, 0, 0);
    }

    for (int i = 1; i <= ITERS; i++) {
        post(&ping, i);
        wait_for(&pong, i);
        if (lost_wakeups > 20) {
            print_num("ABORT early, lost wakeups: ", lost_wakeups);
            print("thr2: FAIL (lost wakeups)\n");
            sc3(SYS_exit_group, 1, 0, 0);
        }
    }
    print_num("iterations: ", ITERS);
    print_num("lost wakeups: ", lost_wakeups);
    if (lost_wakeups)
        print("thr2: FAIL (lost wakeups)\n");
    else
        print("thr2: PASS\n");
    sc3(SYS_exit_group, lost_wakeups ? 1 : 0, 0, 0);
    __builtin_unreachable();
}
