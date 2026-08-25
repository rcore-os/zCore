/* fork-hammer: userspace fork-consistency regression test for Eclipse OS.
 *
 * This caught the real bug fixed by "fix(vm): give fork an mmap_lock": on a
 * kernel without the lock, P7 (fork while other threads mmap/mprotect/munmap
 * and hot-write pages) produced child pages mixing THREE writer generations
 * -- impossible under correct COW -- in under 25 rounds; with the lock it
 * passes 100 rounds clean. P1-P6 pass on both and pin the already-correct
 * behaviours (plain fork, re-fork over COW leaves, quiesced mprotect splits,
 * allocator churn, fork storms).
 *
 * Run it in QEMU without touching the image's userspace:
 *   gcc -static -O2 -o rootfs/x86_64/bin/hammer tools/fork-hammer/hammer.c -lpthread
 *   cargo image --arch x86_64
 *   boot with cmdline ROOTPROC=/bin/hammer (serial shows HAMMER-* lines)
 * Verdict: "HAMMER-END failures=0" -- anything else is a kernel bug.
 */
/* fork-consistency hammer for Eclipse OS.
 *
 * Reproduces (or exonerates) the kernel-side fork memory bugs suspected
 * behind the labwc fork-child SIGSEGV at musl mallocng dequeue (write to
 * 0x8, node with prev=next=0): a page that reads back ZEROED or stale in
 * the child while the parent holds real data.
 *
 * Patterns mirror what the desktop actually does before the crashing fork:
 *   P1 basic fork isolation (bench baseline, should pass)
 *   P2 re-fork (COW leaf re-cloned: hidden-node chains)
 *   P3 JIT W^X mprotect split, then fork (share_count>1 -> eager path)
 *   P4 fork, dirty half, mprotect split of the COW leaf, re-fork  <- prime suspect
 *   P5 fork while threads churn malloc (musl need_locks path)
 *   P6 fork storm with verification on both sides
 *
 * Every buffer is verified page-by-page in child AND parent; a mismatch
 * prints the page index, offset, got/want, and whether the whole page read
 * zero (the smoking gun for a lost COW ancestor page).
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <stdint.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <pthread.h>

#define PAGE 4096

static int failures;

static void fill(uint8_t *b, size_t len, uint32_t seed) {
    for (size_t i = 0; i < len; i++) b[i] = (uint8_t)((seed * 2654435761u + i * 40503u) >> 8);
}

static int verify(const char *who, const char *tag, const uint8_t *b, size_t len, uint32_t seed) {
    for (size_t i = 0; i < len; i++) {
        uint8_t want = (uint8_t)((seed * 2654435761u + i * 40503u) >> 8);
        if (b[i] != want) {
            size_t pg = i / PAGE;
            int allzero = 1;
            for (size_t j = pg * PAGE; j < (pg + 1) * PAGE && j < len; j++)
                if (b[j] != 0) { allzero = 0; break; }
            printf("HAMMER-FAIL %s %s: page %zu off %zu got %02x want %02x%s\n",
                   who, tag, pg, i, b[i], want, allzero ? " (WHOLE PAGE ZERO)" : "");
            failures++;
            return 1;
        }
    }
    return 0;
}

/* fork; child runs f and _exits with its failure count; parent returns
 * child's exit status contribution. */
static void run_child(void (*f)(void)) {
    fflush(stdout);
    pid_t pid = fork();
    if (pid < 0) { printf("HAMMER-FAIL fork errno\n"); failures++; return; }
    if (pid == 0) {
        int before = failures;
        f();
        /* a little malloc churn in the child: the real-world victim died in
         * its first allocator calls between fork and exec */
        for (int i = 0; i < 64; i++) { void *p = malloc(1024 + i * 64); memset(p, 0x5a, 64); free(p); }
        fflush(stdout);
        _exit(failures - before > 250 ? 250 : failures - before);
    }
    int st = 0;
    waitpid(pid, &st, 0);
    if (WIFEXITED(st)) failures += WEXITSTATUS(st);
    else { printf("HAMMER-FAIL child died sig=%d\n", WTERMSIG(st)); failures++; }
}

static uint8_t *A, *B, *C, *J;
#define ASZ (256 * PAGE)
#define BSZ (128 * PAGE)
#define CSZ (128 * PAGE)
#define JSZ (64 * PAGE)

static void child_p1(void) { verify("child", "P1.A", A, ASZ, 1); }

static void child_p2(void) { verify("child", "P2.A", A, ASZ, 1); verify("child", "P2.B", B, BSZ, 2); }

static void child_p3(void) { verify("child", "P3.A", A, ASZ, 1); verify("child", "P3.J", J, JSZ, 3); }

static void child_p4(void) {
    verify("child", "P4.C.lo", C, CSZ / 2, 44);
    verify("child", "P4.C.hi", C + CSZ / 2, CSZ / 2, 4);
    verify("child", "P4.A", A, ASZ, 1);
}

static volatile int churn_on;
static void *churner(void *arg) {
    (void)arg;
    while (churn_on) {
        void *p[16];
        for (int i = 0; i < 16; i++) { p[i] = malloc(512 + i * 96); memset(p[i], 0x33, 128); }
        for (int i = 0; i < 16; i++) free(p[i]);
    }
    return NULL;
}
static void child_p5(void) {
    verify("child", "P5.A", A, ASZ, 1);
    verify("child", "P5.B", B, BSZ, 2);
}

/* ---- v2: concurrent-VMAR-mutation patterns (the labwc/llvmpipe shape) ---- */

#include <signal.h>
#include <setjmp.h>

#define NREG 512
#define REGPG 4
static uint8_t *regs[NREG];

static sigjmp_buf segv_jmp;
static volatile uint64_t segv_addr;
static void on_segv(int sig, siginfo_t *si, void *uc) {
    (void)sig; (void)uc;
    segv_addr = (uint64_t)si->si_addr;
    siglongjmp(segv_jmp, 1);
}

/* JIT-churn thread: continuously mprotect RW->RX->RW and mmap/munmap fresh
 * regions, exactly llvmpipe's W^X storm, concurrent with the forks. */
static volatile int vmchurn_on;
static uint8_t *jitreg;
#define JITPG 32
static void *vm_churner(void *arg) {
    (void)arg;
    while (vmchurn_on) {
        mprotect(jitreg + 8 * PAGE, 8 * PAGE, PROT_READ | PROT_EXEC);
        mprotect(jitreg + 8 * PAGE, 8 * PAGE, PROT_READ | PROT_WRITE);
        void *m = mmap(NULL, 8 * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (m != MAP_FAILED) { memset(m, 0x77, 8 * PAGE); munmap(m, 8 * PAGE); }
    }
    return NULL;
}

/* Hot-writer thread: keeps rewriting one page of two far-apart regions with a
 * generation stamp (every u64 = gen). A child page must read as gen G or
 * G±few or a single split point -- ZERO or garbage is kernel corruption. */
static volatile int hot_on;
static void *hot_writer(void *arg) {
    (void)arg;
    uint64_t gen = 1;
    uint64_t *early = (uint64_t *)regs[5];
    uint64_t *late = (uint64_t *)regs[NREG - 5];
    while (hot_on) {
        gen++;
        for (int i = 0; i < 512; i++) early[i] = gen;
        for (int i = 0; i < 512; i++) late[i] = gen;
    }
    return NULL;
}

static void verify_regs_child(const char *tag) {
    /* child-side: catch SIGSEGV so a LOST mapping reports instead of dying */
    struct sigaction sa = {0};
    sa.sa_sigaction = on_segv;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGSEGV, &sa, NULL);
    for (int r = 0; r < NREG; r += 3) { /* subset: keep each round fast under TCG */
        if (r == 5 || r == NREG - 5) continue; /* hot pages: checked below */
        if (sigsetjmp(segv_jmp, 1)) {
            printf("HAMMER-FAIL child %s: region %d UNMAPPED (SIGSEGV at %#llx)\n",
                   tag, r, (unsigned long long)segv_addr);
            failures++;
            continue;
        }
        verify("child", tag, regs[r], REGPG * PAGE, 100 + r);
    }
    /* hot pages: all u64s must be plausible generations (nonzero), allowing
     * one torn boundary; an all-zero page is the lost-content signature */
    for (int h = 0; h < 2; h++) {
        uint64_t *p = (uint64_t *)regs[h == 0 ? 5 : NREG - 5];
        if (sigsetjmp(segv_jmp, 1)) {
            printf("HAMMER-FAIL child %s: HOT region %d UNMAPPED\n", tag, h);
            failures++;
            continue;
        }
        int zeros = 0, vals = 0;
        uint64_t seen[3] = {0, 0, 0};
        for (int i = 0; i < 512; i++) {
            uint64_t v = p[i];
            if (v == 0) { zeros++; continue; }
            int known = 0;
            for (int k = 0; k < vals; k++) if (seen[k] == v) known = 1;
            if (!known && vals < 3) seen[vals++] = v;
        }
        if (zeros == 512) {
            printf("HAMMER-FAIL child %s: HOT page %d WHOLE PAGE ZERO\n", tag, h);
            failures++;
        } else if (zeros > 0) {
            printf("HAMMER-FAIL child %s: HOT page %d has %d zero u64s (partial loss)\n",
                   tag, h, zeros);
            failures++;
        } else if (vals > 2) {
            printf("HAMMER-FAIL child %s: HOT page %d mixes %d generations (torn snapshot)\n",
                   tag, h, vals);
            failures++;
        }
    }
    /* the JIT region's stable low pages must hold their pattern */
    if (sigsetjmp(segv_jmp, 1)) {
        printf("HAMMER-FAIL child %s: JIT region UNMAPPED\n", tag);
        failures++;
    } else {
        verify("child", "JIT.lo", jitreg, 8 * PAGE, 77);
        verify("child", "JIT.hi", jitreg + 16 * PAGE, 16 * PAGE, 78);
    }
    signal(SIGSEGV, SIG_DFL);
}

static void child_p7(void) { verify_regs_child("P7"); }

int main(void) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    printf("HAMMER-START\n");

    /* P1: plain fork isolation */
    A = malloc(ASZ); fill(A, ASZ, 1);
    run_child(child_p1);
    verify("parent", "P1.A-after", A, ASZ, 1);
    printf("HAMMER P1 done failures=%d\n", failures);

    /* P2: re-fork — the parent's mappings are now COW leaves of fork 1 */
    B = malloc(BSZ); fill(B, BSZ, 2);
    run_child(child_p2);
    run_child(child_p2);
    verify("parent", "P2.A-after", A, ASZ, 1);
    verify("parent", "P2.B-after", B, BSZ, 2);
    printf("HAMMER P2 done failures=%d\n", failures);

    /* P3: JIT W^X pattern — mprotect split, then fork */
    J = mmap(NULL, JSZ, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    fill(J, JSZ, 3);
    mprotect(J + 16 * PAGE, 16 * PAGE, PROT_READ | PROT_EXEC); /* split: RW | RX | RW */
    run_child(child_p3);
    verify("parent", "P3.J-after", J, JSZ, 3);
    mprotect(J + 16 * PAGE, 16 * PAGE, PROT_READ | PROT_WRITE);
    verify("parent", "P3.J-after-rw", J, JSZ, 3);
    run_child(child_p3);
    printf("HAMMER P3 done failures=%d\n", failures);

    /* P4: fork, dirty HALF of a buffer (leaf frames), leave half in the
     * hidden node, mprotect-split the mapping, re-fork. */
    C = mmap(NULL, CSZ, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    fill(C, CSZ, 4);
    run_child(child_p1);              /* fork 1: C becomes a COW leaf   */
    fill(C, CSZ / 2, 44);             /* dirty low half AFTER fork 1    */
    mprotect(C + 48 * PAGE, 16 * PAGE, PROT_READ); /* split mid-buffer */
    run_child(child_p4);              /* fork 2: the suspect            */
    mprotect(C + 48 * PAGE, 16 * PAGE, PROT_READ | PROT_WRITE);
    verify("parent", "P4.C.lo-after", C, CSZ / 2, 44);
    verify("parent", "P4.C.hi-after", C + CSZ / 2, CSZ / 2, 4);
    printf("HAMMER P4 done failures=%d\n", failures);

    /* P5: fork while 4 threads churn the allocator */
    churn_on = 1;
    pthread_t th[4];
    for (int i = 0; i < 4; i++) pthread_create(&th[i], NULL, churner, NULL);
    for (int r = 0; r < 5; r++) run_child(child_p5);
    churn_on = 0;
    for (int i = 0; i < 4; i++) pthread_join(th[i], NULL);
    printf("HAMMER P5 done failures=%d\n", failures);

    /* P6: fork storm */
    for (int r = 0; r < 20; r++) run_child(child_p2);
    verify("parent", "P6.A-after", A, ASZ, 1);
    verify("parent", "P6.B-after", B, BSZ, 2);
    printf("HAMMER P6 done failures=%d\n", failures);

    /* P7: the labwc shape -- ~520 mappings, hot writer threads, and a
     * llvmpipe-style W^X/mmap churn thread ALL mutating the address space
     * while the main thread forks. This is the pattern the v1 hammer lacked
     * and the one the real crash rode in on. */
    for (int r = 0; r < NREG; r++) {
        regs[r] = mmap(NULL, REGPG * PAGE, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        fill(regs[r], REGPG * PAGE, 100 + r);
    }
    jitreg = mmap(NULL, JITPG * PAGE, PROT_READ | PROT_WRITE,
                  MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    fill(jitreg, 8 * PAGE, 77);
    fill(jitreg + 8 * PAGE, 8 * PAGE, 79);
    fill(jitreg + 16 * PAGE, 16 * PAGE, 78);
    hot_on = 1; vmchurn_on = 1; churn_on = 1;
    pthread_t hw, vc, mc;
    pthread_create(&hw, NULL, hot_writer, NULL);
    pthread_create(&vc, NULL, vm_churner, NULL);
    pthread_create(&mc, NULL, churner, NULL);
    for (int round = 0; round < 100; round++) {
        run_child(child_p7);
        /* parent-side spot check of a few static regions each round */
        for (int r = round % 7; r < NREG; r += 97)
            if (r != 5 && r != NREG - 5)
                verify("parent", "P7.static", regs[r], REGPG * PAGE, 100 + r);
    }
    hot_on = 0; vmchurn_on = 0; churn_on = 0;
    pthread_join(hw, NULL); pthread_join(vc, NULL); pthread_join(mc, NULL);
    printf("HAMMER P7 done failures=%d\n", failures);

    printf("HAMMER-END failures=%d\n", failures);
    fflush(stdout);
    /* keep the console process alive so the kernel does not reap the root
     * process the instant the run ends */
    for (;;) pause();
}
