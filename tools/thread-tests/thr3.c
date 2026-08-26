/*
 * Faithful reproduction of sysbench's startup synchronization on Eclipse OS.
 *
 * Implements, with raw syscalls only, the exact kernel-visible sequence that
 * sysbench 1.0.20 + musl 1.2.x produce between "Initializing worker
 * threads..." and "Threads started!":
 *
 *   - pthread_create: stack = mmap(PROT_NONE) + mprotect(RW) (guard page),
 *     parent writes the TLS/pthread block inside the mapping, then
 *     clone(0x7d0f00) with ptid/ctid like musl.
 *   - sb_barrier_wait: pthread_mutex (normal) + musl's private condvar
 *     algorithm (per-waiter node with barrier word, _c_lock spinlock with
 *     futex fallback, FUTEX_WAIT / FUTEX_WAKE / FUTEX_REQUEUE).
 *
 * Main thread and N workers all meet at the barrier; the last arrival
 * broadcasts. Repeats many rounds to catch races. Any lost wakeup hangs a
 * round; a watchdog (futex timeout) detects it and reports FAIL.
 *
 * Build (bare metal): gcc -static -no-pie -nostdlib -O1 -o thr3 thr3.c
 * Build (libos):      gcc -DVIA_CALL -static-pie -nostdlib -fPIC -O1 -o thr3 thr3.c
 */

typedef unsigned long u64;
typedef long i64;
typedef unsigned int u32;

__attribute__((used, section(".rodata.rcore"))) u64 rcore_syscall_entry = 0xdeadbeef;

#ifdef VIA_CALL
#define SYSCALL_INSN "call *rcore_syscall_entry(%%rip)"
#else
#define SYSCALL_INSN "syscall"
#endif

#define SYS_write 1
#define SYS_mmap 9
#define SYS_mprotect 10
#define SYS_clone 56
#define SYS_exit 60
#define SYS_exit_group 231
#define SYS_futex 202

#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#define FUTEX_REQUEUE 3
#define FUTEX_PRIVATE 128

#define PROT_NONE 0
#define PROT_RW 3
#define MAP_PRIV_ANON 0x22

#define NWORKERS 3
#ifdef QUICK_TEST
#define ROUNDS 200 /* rootfs dev build via xtask: feedback en segundos */
#else
#define ROUNDS 3000
#endif
#define WATCHDOG_NS (2000 * 1000 * 1000UL) /* 2 s: round must finish well before */
#define PROGRESS_EVERY 50

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

/* ---- musl-style atomics / futex helpers ---- */
#define a_cas(p, t, s) __sync_val_compare_and_swap((p), (t), (s))
#define a_swap(p, v) __atomic_exchange_n((p), (v), __ATOMIC_SEQ_CST)
#define a_store(p, v) __atomic_store_n((p), (v), __ATOMIC_SEQ_CST)
#define a_load(p) __atomic_load_n((p), __ATOMIC_SEQ_CST)

static volatile int watchdog_hit;
static volatile int rounds_done;
static volatile int worker_started[3];

/* musl __wait with a watchdog timeout so a lost wakeup is detected
 * instead of hanging the test forever. */
/* label for stall diagnostics */
static void wait_w2(volatile int *addr, volatile int *waiters, int val, const char *what);
static void wait_w(volatile int *addr, volatile int *waiters, int val)
{
    wait_w2(addr, waiters, val, "?");
}
static void wait_w2(volatile int *addr, volatile int *waiters, int val, const char *what)
{
    int spins = 100;
    while (spins-- && a_load(addr) == val)
        ;
    if (waiters)
        __atomic_fetch_add(waiters, 1, __ATOMIC_SEQ_CST);
    while (a_load(addr) == val) {
        struct timespec ts = {0, 0};
        ts.tv_sec = WATCHDOG_NS / 1000000000UL;
        ts.tv_nsec = WATCHDOG_NS % 1000000000UL;
        i64 r = sc6(SYS_futex, (i64)addr, FUTEX_WAIT | FUTEX_PRIVATE, val,
                    (i64)&ts, 0, 0);
        if (r == -110 /*ETIMEDOUT*/ && a_load(addr) == val) {
            /* still the old value after 2s: real stall, keep waiting */
            print(what);
            print_num(" stall round=", rounds_done);
            print_num(" addr_lo=", ((i64)addr) & 0xffff);
            print_num(" val=", a_load(addr));
            continue;
        }
        if (r == -110 && a_load(addr) != val) {
            /* value changed but we were never woken: LOST WAKEUP */
            watchdog_hit++;
            break;
        }
    }
    if (waiters)
        __atomic_fetch_add(waiters, -1, __ATOMIC_SEQ_CST);
}

static void wake_w(volatile int *addr, int cnt)
{
    sc6(SYS_futex, (i64)addr, FUTEX_WAKE | FUTEX_PRIVATE, cnt, 0, 0, 0);
}

/* ---- musl pthread_mutex (type normal) ---- */
typedef struct {
    volatile int lock;
    volatile int waiters;
} mutex_t;

static void mutex_lock(mutex_t *m)
{
    if (a_cas(&m->lock, 0, 1) == 0)
        return;
    /* musl normal-mutex slow path: value 1 = locked, INT_MIN bit = contended */
    while (a_swap(&m->lock, 2) != 0)
        wait_w2(&m->lock, &m->waiters, 2, "mutex");
}

static void mutex_unlock(mutex_t *m)
{
    if (a_swap(&m->lock, 0) == 2 || a_load(&m->waiters))
        wake_w(&m->lock, 1);
}

/* ---- musl private condvar (pthread_cond_timedwait.c, simplified to the
 *      paths a barrier hits: wait + broadcast) ---- */
enum { WAITING, SIGNALED, LEAVING };

struct cnode {
    struct cnode *prev, *next;
    volatile int state, barrier;
    volatile int *notify;
};

typedef struct {
    volatile int c_lock;
    volatile int c_lock_waiters;
    struct cnode *head, *tail;
} cond_t;

static void cv_lock(cond_t *c)
{
    if (a_cas(&c->c_lock, 0, 1)) {
        a_cas(&c->c_lock, 1, 2);
        do
            wait_w2(&c->c_lock, &c->c_lock_waiters, 2, "clock");
        while (a_cas(&c->c_lock, 0, 2));
    }
}

static void cv_unlock(cond_t *c)
{
    if (a_swap(&c->c_lock, 0) == 2)
        wake_w(&c->c_lock, 1);
}

static void unlock_requeue(volatile int *l, volatile int *r)
{
    a_store(l, 0);
    sc6(SYS_futex, (i64)l, FUTEX_REQUEUE | FUTEX_PRIVATE, 0, 1, (i64)r, 0);
}

static void cond_wait(cond_t *c, mutex_t *m)
{
    struct cnode node = {0};
    int oldstate;

    cv_lock(c);
    node.barrier = 2;
    node.state = WAITING;
    node.next = c->head;
    c->head = &node;
    if (!c->tail)
        c->tail = &node;
    else
        node.next->prev = &node;
    cv_unlock(c);

    mutex_unlock(m);

    do
        wait_w2(&node.barrier, 0, 2, "nodebar");
    while (a_load(&node.barrier) == 2);

    oldstate = a_cas(&node.state, WAITING, LEAVING);

    if (oldstate == WAITING) {
        /* timed out / spurious: remove self (not hit in this test) */
        cv_lock(c);
        if (c->head == &node)
            c->head = node.next;
        else if (node.prev)
            node.prev->next = node.next;
        if (c->tail == &node)
            c->tail = node.prev;
        else if (node.next)
            node.next->prev = node.prev;
        cv_unlock(c);
        if (node.notify && __atomic_fetch_add(node.notify, -1, __ATOMIC_SEQ_CST) == 1)
            wake_w(node.notify, 1);
    } else {
        /* Lock barrier first to control wake order. */
        if (a_cas(&node.barrier, 0, 1)) {
            a_cas(&node.barrier, 1, 2);
            while (a_load(&node.barrier) == 2)
                wait_w2(&node.barrier, 0, 2, "nodebar2");
        }
    }

    mutex_lock(m);

    if (oldstate != WAITING) {
        /* musl's _m_waiters accounting: keep the mutex aware of requeued
         * waiters so unlock issues the FUTEX_WAKE that wakes them. */
        if (!node.next)
            __atomic_fetch_add(&m->waiters, 1, __ATOMIC_SEQ_CST);
        if (node.prev)
            unlock_requeue(&node.prev->barrier, &m->lock);
        else
            __atomic_fetch_add(&m->waiters, -1, __ATOMIC_SEQ_CST);
    }
}

static void cond_broadcast(cond_t *c)
{
    struct cnode *p, *first = 0;
    volatile int ref = 0;
    int cur;

    cv_lock(c);
    for (p = c->tail; p; p = p->prev) {
        if (a_cas(&p->state, WAITING, SIGNALED) != WAITING) {
            __atomic_fetch_add(&ref, 1, __ATOMIC_SEQ_CST);
            p->notify = &ref;
        } else if (!first) {
            first = p;
        }
    }
    c->head = 0;
    c->tail = 0;
    cv_unlock(c);

    while ((cur = a_load(&ref)))
        wait_w2(&ref, 0, cur, "ref");

    if (first) {
        /* unlock(first->barrier) */
        if (a_swap(&first->barrier, 0) == 2)
            wake_w(&first->barrier, 1);
    }
}

/* ---- sb_barrier (sysbench sb_barrier.c) ---- */
typedef struct {
    mutex_t mutex;
    cond_t cond;
    unsigned init_count, count, serial;
} sb_barrier_t;

static void sb_barrier_init(sb_barrier_t *b, unsigned count)
{
    b->mutex.lock = 0;
    b->mutex.waiters = 0;
    b->cond.c_lock = 0;
    b->cond.head = b->cond.tail = 0;
    b->init_count = b->count = count;
    b->serial = 0;
}

static int sb_barrier_wait(sb_barrier_t *b)
{
    int res = 0;
    mutex_lock(&b->mutex);
    if (!--b->count) {
        b->serial++;
        b->count = b->init_count;
        res = 1; /* serial thread: runs the callback */
        cond_broadcast(&b->cond);
        rounds_done = b->serial; /* "Threads started!" */
        mutex_unlock(&b->mutex);
    } else {
        unsigned serial = b->serial;
        do
            cond_wait(&b->cond, &b->mutex);
        while (serial == b->serial);
        mutex_unlock(&b->mutex);
    }
    return res;
}

/* ---- workers ---- */
static sb_barrier_t barrier;

static int worker_fn(void *arg)
{
    int id = (int)(i64)arg;
    worker_started[id] = 1;
    for (int r = 0; r < ROUNDS; r++)
        sb_barrier_wait(&barrier);
    sc3(SYS_exit, 0, 0, 0);
    return 0;
}

static i64 musl_clone(int (*fn)(void *), void *stack_top, void *arg, int *ptid,
                      void *tls, int *ctid)
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
                     : "0"((i64)SYS_clone), "D"((i64)0x7d0f00), "S"(sp),
                       "d"(ptid), "r"(r10), "r"(r8), "r"(r9)
                     : "rcx", "r11", "memory");
    return ret;
}

static int tids[NWORKERS];
static int list_lock;

__attribute__((used)) static void run_main(void);

/* Process entry: the kernel hands over rsp 16-byte aligned (SysV process
 * entry ABI), but gcc compiles ordinary functions assuming entry via call
 * (rsp ≡ 8 mod 16). Realign before entering C code, or SSE spills
 * (movaps) fault with #GP. */
__attribute__((naked)) void _start(void)
{
    __asm__ volatile("and $-16, %rsp\n\t"
                     "xor %ebp, %ebp\n\t"
                     "call run_main\n\t"
                     "hlt");
}

__attribute__((used)) static void run_main(void)
{
    print("thr3: sysbench-barrier repro start\n");

    sb_barrier_init(&barrier, 1 + NWORKERS); /* main + workers, like sysbench */

    for (int i = 0; i < NWORKERS; i++) {
        /* musl pthread_create stack: PROT_NONE map, then mprotect past the
         * guard page; TLS block at the top, written by the parent. */
        u64 size = 64 * 1024 + 4096;
        i64 map = sc6(SYS_mmap, 0, size, PROT_NONE, MAP_PRIV_ANON, -1, 0);
        if (map < 0) {
            print_num("FAIL mmap: ", map);
            sc3(SYS_exit_group, 1, 0, 0);
        }
        i64 mp = sc3(SYS_mprotect, map + 4096, size - 4096, PROT_RW);
        if (mp < 0) {
            print_num("FAIL mprotect: ", mp);
            sc3(SYS_exit_group, 1, 0, 0);
        }
        /* parent writes the "pthread/TLS" block at the top, like __copy_tls */
        u64 *tls = (u64 *)(map + size - 512);
        for (int k = 0; k < 64; k++)
            tls[k] = 0;
        tls[0] = (u64)tls;
        i64 r = musl_clone(worker_fn, (void *)((u64)tls & -64UL), (void *)(i64)i,
                           &tids[i], tls, &list_lock);
        if (r <= 0) {
            print_num("FAIL clone: ", r);
            sc3(SYS_exit_group, 1, 0, 0);
        }
    }

    print("workers created\n");
    print("barrier stress: ");
    print_num("", ROUNDS);
    print(" rounds (progress every ");
    print_num("", PROGRESS_EVERY);
    print(")\n");
    for (int r = 0; r < ROUNDS; r++) {
        sb_barrier_wait(&barrier);
        if (r == 0)
            print("barrier round 0 ok\n");
        else if ((r + 1) % PROGRESS_EVERY == 0) {
            print_num("barrier round ", r + 1);
            print("\n");
        }
    }

    print_num("rounds completed: ", rounds_done);
    print_num("lost wakeups: ", watchdog_hit);
    int ok = rounds_done >= ROUNDS && !watchdog_hit;
    for (int i = 0; i < NWORKERS; i++)
        if (!worker_started[i]) {
            print_num("FAIL: worker never ran: ", i);
            ok = 0;
        }
    print(ok ? "thr3: PASS\n" : "thr3: FAIL\n");
    sc3(SYS_exit_group, ok ? 0 : 1, 0, 0);
    __builtin_unreachable();
}
