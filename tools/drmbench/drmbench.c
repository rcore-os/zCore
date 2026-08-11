// drmbench — Eclipse vs Linux graphics benchmark on the raw DRM/KMS path.
//
// Deliberately self-contained: it talks to /dev/dri/cardN with hand-declared
// ioctl structures and NO libdrm, so the same binary runs on Eclipse and on any
// Linux without installing a thing. That matters because the comparison is only
// meaningful when both sides run the *identical* workload; a benchmark that
// needs packages one side lacks measures the packaging, not the kernel.
//
// It also needs no compositor. Every number here comes from ioctls Eclipse
// implements directly, which is what makes this runnable on an image where the
// Wayland stack is absent.
//
// METHODOLOGY — read before quoting any number:
//
//   Run BOTH sides under the same QEMU invocation (same -accel, -smp, -m and
//   the same virtual GPU), booting Eclipse in one and a Linux distro in the
//   other. Only then is the kernel the single variable. Comparing an emulated
//   Eclipse against a native Linux measures virtualization overhead, not the
//   kernel, and the difference will be dominated by the emulator.
//
//   Under TCG (no KVM) these numbers describe QEMU's emulator, not the guest
//   kernel. Use KVM on both sides.
//
// Output is `key=value` lines on stdout (one metric per line) so two runs can
// be diffed mechanically; human notes go to stderr.
//
// Build:  cc -O2 -o drmbench drmbench.c
// Run:    ./drmbench [/dev/dri/card0] [seconds-per-bench]

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

// ── DRM ABI (subset), declared here so no kernel headers are required ────────

#define DRM_IOCTL_BASE 'd'
#define DRM_IO(nr) _IO(DRM_IOCTL_BASE, nr)
#define DRM_IOW(nr, type) _IOW(DRM_IOCTL_BASE, nr, type)
#define DRM_IOWR(nr, type) _IOWR(DRM_IOCTL_BASE, nr, type)

struct drm_set_client_cap {
    uint64_t capability;
    uint64_t value;
};

struct drm_mode_card_res {
    uint64_t fb_id_ptr, crtc_id_ptr, connector_id_ptr, encoder_id_ptr;
    uint32_t count_fbs, count_crtcs, count_connectors, count_encoders;
    uint32_t min_width, max_width, min_height, max_height;
};

struct drm_mode_modeinfo {
    uint32_t clock;
    uint16_t hdisplay, hsync_start, hsync_end, htotal, hskew;
    uint16_t vdisplay, vsync_start, vsync_end, vtotal, vscan;
    uint32_t vrefresh, flags, type;
    char name[32];
};

struct drm_mode_get_connector {
    uint64_t encoders_ptr, modes_ptr, props_ptr, prop_values_ptr;
    uint32_t count_modes, count_props, count_encoders;
    uint32_t encoder_id, connector_id, connector_type, connector_type_id;
    uint32_t connection, mm_width, mm_height, subpixel;
    uint32_t pad;
};

struct drm_mode_get_encoder {
    uint32_t encoder_id, encoder_type, crtc_id;
    uint32_t possible_crtcs, possible_clones;
};

struct drm_mode_crtc {
    uint64_t set_connectors_ptr;
    uint32_t count_connectors, crtc_id, fb_id, x, y, gamma_size, mode_valid;
    struct drm_mode_modeinfo mode;
};

struct drm_mode_create_dumb {
    uint32_t height, width, bpp, flags;
    uint32_t handle, pitch;
    uint64_t size;
};

struct drm_mode_map_dumb {
    uint32_t handle, pad;
    uint64_t offset;
};

struct drm_mode_destroy_dumb {
    uint32_t handle;
};

struct drm_mode_fb_cmd2 {
    uint32_t fb_id, width, height, pixel_format, flags;
    uint32_t handles[4], pitches[4], offsets[4];
    uint64_t modifier[4];
};

struct drm_mode_crtc_page_flip {
    uint32_t crtc_id, fb_id, flags, reserved;
    uint64_t user_data;
};

struct drm_mode_cursor {
    uint32_t flags, crtc_id;
    int32_t x, y;
    uint32_t width, height, handle;
};

struct drm_mode_get_plane_res {
    uint64_t plane_id_ptr;
    uint32_t count_planes;
};

struct drm_mode_get_plane {
    uint32_t plane_id, crtc_id, fb_id;
    uint32_t possible_crtcs, gamma_size, count_format_types;
    uint64_t format_type_ptr;
};

struct drm_mode_obj_get_properties {
    uint64_t props_ptr, prop_values_ptr;
    uint32_t count_props, obj_id, obj_type;
};

struct drm_mode_get_property {
    uint64_t values_ptr, enum_blob_ptr;
    uint32_t prop_id, flags;
    char name[32];
    uint32_t count_values, count_enum_blobs;
};

struct drm_mode_atomic {
    uint32_t flags, count_objs;
    uint64_t objs_ptr, count_props_ptr, props_ptr, prop_values_ptr;
    uint64_t reserved, user_data;
};

struct drm_event {
    uint32_t type, length;
};

union drm_wait_vblank_request {
    struct {
        uint32_t type, sequence;
        unsigned long signal;
    } request;
    struct {
        uint32_t type, sequence;
        long tval_sec, tval_usec;
    } reply;
};

#define DRM_IOCTL_SET_MASTER DRM_IO(0x1e)
#define DRM_IOCTL_DROP_MASTER DRM_IO(0x1f)
#define DRM_IOCTL_SET_CLIENT_CAP DRM_IOW(0x0d, struct drm_set_client_cap)
#define DRM_IOCTL_WAIT_VBLANK DRM_IOWR(0x3a, union drm_wait_vblank_request)
#define DRM_IOCTL_MODE_GETRESOURCES DRM_IOWR(0xa0, struct drm_mode_card_res)
#define DRM_IOCTL_MODE_GETCRTC DRM_IOWR(0xa1, struct drm_mode_crtc)
#define DRM_IOCTL_MODE_SETCRTC DRM_IOWR(0xa2, struct drm_mode_crtc)
#define DRM_IOCTL_MODE_CURSOR DRM_IOWR(0xa3, struct drm_mode_cursor)
#define DRM_IOCTL_MODE_GETENCODER DRM_IOWR(0xa6, struct drm_mode_get_encoder)
#define DRM_IOCTL_MODE_GETCONNECTOR DRM_IOWR(0xa7, struct drm_mode_get_connector)
#define DRM_IOCTL_MODE_RMFB DRM_IOWR(0xaf, unsigned int)
#define DRM_IOCTL_MODE_PAGE_FLIP DRM_IOWR(0xb0, struct drm_mode_crtc_page_flip)
#define DRM_IOCTL_MODE_CREATE_DUMB DRM_IOWR(0xb2, struct drm_mode_create_dumb)
#define DRM_IOCTL_MODE_MAP_DUMB DRM_IOWR(0xb3, struct drm_mode_map_dumb)
#define DRM_IOCTL_MODE_DESTROY_DUMB DRM_IOWR(0xb4, struct drm_mode_destroy_dumb)
#define DRM_IOCTL_MODE_ADDFB2 DRM_IOWR(0xb8, struct drm_mode_fb_cmd2)
#define DRM_IOCTL_MODE_GETPROPERTY DRM_IOWR(0xaa, struct drm_mode_get_property)
#define DRM_IOCTL_MODE_GETPLANERESOURCES DRM_IOWR(0xb5, struct drm_mode_get_plane_res)
#define DRM_IOCTL_MODE_GETPLANE DRM_IOWR(0xb6, struct drm_mode_get_plane)
#define DRM_IOCTL_MODE_OBJ_GETPROPERTIES DRM_IOWR(0xb9, struct drm_mode_obj_get_properties)
#define DRM_IOCTL_MODE_ATOMIC DRM_IOWR(0xbc, struct drm_mode_atomic)

#define DRM_MODE_OBJECT_CRTC 0xcccccccc
#define DRM_MODE_OBJECT_CONNECTOR 0xc0c0c0c0
#define DRM_MODE_OBJECT_PLANE 0xeeeeeeee
#define DRM_MODE_ATOMIC_TEST_ONLY 0x0100
#define DRM_MODE_ATOMIC_NONBLOCK 0x0200

#define DRM_MODE_PAGE_FLIP_EVENT 0x01
#define DRM_MODE_CONNECTED 1
#define DRM_CLIENT_CAP_UNIVERSAL_PLANES 2
#define DRM_CLIENT_CAP_ATOMIC 3
#define DRM_FORMAT_XRGB8888 0x34325258
#define DRM_VBLANK_RELATIVE 0x1

// ── helpers ──────────────────────────────────────────────────────────────────

static int g_fd = -1;
static double g_secs = 3.0;

static uint64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

// EINTR is routine here (the flip event and the tick both interrupt), and a
// benchmark that miscounts an interrupted call as a failure would report a
// kernel as broken when it merely got a signal.
static int xioctl(int fd, unsigned long req, void *arg) {
    int r;
    do {
        r = ioctl(fd, req, arg);
    } while (r == -1 && errno == EINTR);
    return r;
}

// Newton-Raphson square root, so the binary needs no libm. The guest image may
// not ship one, and a benchmark that fails to link on the machine under test is
// worth less than a slightly longer helper here.
static double approx_sqrt(double x) {
    if (x <= 0)
        return 0;
    double g = x > 1 ? x / 2 : 1;
    for (int i = 0; i < 40; i++)
        g = 0.5 * (g + x / g);
    return g;
}

static void emit(const char *key, double value, const char *unit) {
    printf("%s=%.2f %s\n", key, value, unit);
    fflush(stdout);
}

static void skip(const char *key, const char *why) {
    // A metric the kernel does not implement is reported, not silently
    // dropped: "unsupported" is itself a comparison result.
    printf("%s=unsupported (%s)\n", key, why);
    fflush(stdout);
}

// ── mode setup ───────────────────────────────────────────────────────────────

struct fb {
    uint32_t handle, fb_id, pitch;
    uint64_t size;
    void *map;
};

static uint32_t g_crtc_id, g_conn_id;
static struct drm_mode_modeinfo g_mode;

static int make_fb(struct fb *f, uint32_t w, uint32_t h, int want_map) {
    struct drm_mode_create_dumb c;
    memset(&c, 0, sizeof c);
    c.width = w;
    c.height = h;
    c.bpp = 32;
    if (xioctl(g_fd, DRM_IOCTL_MODE_CREATE_DUMB, &c) < 0)
        return -1;
    f->handle = c.handle;
    f->pitch = c.pitch;
    f->size = c.size;
    f->map = NULL;

    struct drm_mode_fb_cmd2 fb;
    memset(&fb, 0, sizeof fb);
    fb.width = w;
    fb.height = h;
    fb.pixel_format = DRM_FORMAT_XRGB8888;
    fb.handles[0] = c.handle;
    fb.pitches[0] = c.pitch;
    if (xioctl(g_fd, DRM_IOCTL_MODE_ADDFB2, &fb) < 0)
        return -1;
    f->fb_id = fb.fb_id;

    if (want_map) {
        struct drm_mode_map_dumb m;
        memset(&m, 0, sizeof m);
        m.handle = c.handle;
        if (xioctl(g_fd, DRM_IOCTL_MODE_MAP_DUMB, &m) == 0) {
            void *p = mmap(NULL, c.size, PROT_READ | PROT_WRITE, MAP_SHARED,
                           g_fd, (off_t)m.offset);
            if (p != MAP_FAILED)
                f->map = p;
        }
    }
    return 0;
}

static void free_fb(struct fb *f) {
    if (f->map)
        munmap(f->map, f->size);
    if (f->fb_id) {
        unsigned int id = f->fb_id;
        xioctl(g_fd, DRM_IOCTL_MODE_RMFB, &id);
    }
    if (f->handle) {
        struct drm_mode_destroy_dumb d;
        memset(&d, 0, sizeof d);
        d.handle = f->handle;
        xioctl(g_fd, DRM_IOCTL_MODE_DESTROY_DUMB, &d);
    }
    memset(f, 0, sizeof *f);
}

// Find a connected connector, its mode and a usable CRTC.
static int setup_mode(void) {
    struct drm_mode_card_res res;
    memset(&res, 0, sizeof res);
    if (xioctl(g_fd, DRM_IOCTL_MODE_GETRESOURCES, &res) < 0)
        return -1;
    if (!res.count_connectors || !res.count_crtcs)
        return -1;

    uint32_t conns[32], crtcs[32], encs[32], fbs[32];
    if (res.count_connectors > 32)
        res.count_connectors = 32;
    if (res.count_crtcs > 32)
        res.count_crtcs = 32;
    if (res.count_encoders > 32)
        res.count_encoders = 32;
    if (res.count_fbs > 32)
        res.count_fbs = 32;
    res.connector_id_ptr = (uint64_t)(uintptr_t)conns;
    res.crtc_id_ptr = (uint64_t)(uintptr_t)crtcs;
    res.encoder_id_ptr = (uint64_t)(uintptr_t)encs;
    res.fb_id_ptr = (uint64_t)(uintptr_t)fbs;
    if (xioctl(g_fd, DRM_IOCTL_MODE_GETRESOURCES, &res) < 0)
        return -1;

    for (uint32_t i = 0; i < res.count_connectors; i++) {
        struct drm_mode_get_connector gc;
        memset(&gc, 0, sizeof gc);
        gc.connector_id = conns[i];
        if (xioctl(g_fd, DRM_IOCTL_MODE_GETCONNECTOR, &gc) < 0)
            continue;
        if (gc.connection != DRM_MODE_CONNECTED || !gc.count_modes)
            continue;

        struct drm_mode_modeinfo modes[64];
        uint32_t eids[32];
        if (gc.count_modes > 64)
            gc.count_modes = 64;
        if (gc.count_encoders > 32)
            gc.count_encoders = 32;
        gc.modes_ptr = (uint64_t)(uintptr_t)modes;
        gc.encoders_ptr = (uint64_t)(uintptr_t)eids;
        gc.count_props = 0;
        gc.props_ptr = 0;
        gc.prop_values_ptr = 0;
        if (xioctl(g_fd, DRM_IOCTL_MODE_GETCONNECTOR, &gc) < 0)
            continue;
        if (!gc.count_modes)
            continue;

        g_conn_id = conns[i];
        g_mode = modes[0]; // preferred mode is first

        // Prefer the encoder's current CRTC; fall back to the first.
        g_crtc_id = 0;
        if (gc.encoder_id) {
            struct drm_mode_get_encoder ge;
            memset(&ge, 0, sizeof ge);
            ge.encoder_id = gc.encoder_id;
            if (xioctl(g_fd, DRM_IOCTL_MODE_GETENCODER, &ge) == 0 && ge.crtc_id)
                g_crtc_id = ge.crtc_id;
        }
        if (!g_crtc_id)
            g_crtc_id = crtcs[0];
        return 0;
    }
    return -1;
}

static int set_crtc(uint32_t fb_id) {
    struct drm_mode_crtc sc;
    memset(&sc, 0, sizeof sc);
    sc.crtc_id = g_crtc_id;
    sc.fb_id = fb_id;
    sc.set_connectors_ptr = (uint64_t)(uintptr_t)&g_conn_id;
    sc.count_connectors = 1;
    sc.mode = g_mode;
    sc.mode_valid = 1;
    return xioctl(g_fd, DRM_IOCTL_MODE_SETCRTC, &sc);
}

// Drain one page-flip completion event. Returns 0 on success.
static int wait_flip_event(void) {
    char buf[256];
    ssize_t n = read(g_fd, buf, sizeof buf);
    return n > 0 ? 0 : -1;
}

// ── benchmarks ───────────────────────────────────────────────────────────────

// 1. Page-flip rate: the core presentation path. Each iteration queues a flip
//    with an event and waits for its completion, so this measures the full
//    round trip the compositor pays per frame — not just ioctl entry cost.
static void bench_pageflip(struct fb *a, struct fb *b) {
    uint64_t t0 = now_ns(), deadline = t0 + (uint64_t)(g_secs * 1e9);
    uint64_t flips = 0, fails = 0;
    struct fb *front = a, *back = b;
    while (now_ns() < deadline) {
        struct drm_mode_crtc_page_flip pf;
        memset(&pf, 0, sizeof pf);
        pf.crtc_id = g_crtc_id;
        pf.fb_id = back->fb_id;
        pf.flags = DRM_MODE_PAGE_FLIP_EVENT;
        if (xioctl(g_fd, DRM_IOCTL_MODE_PAGE_FLIP, &pf) < 0) {
            if (++fails > 16) {
                skip("pageflip_per_sec", strerror(errno));
                return;
            }
            continue;
        }
        if (wait_flip_event() < 0)
            break;
        flips++;
        struct fb *t = front;
        front = back;
        back = t;
    }
    double el = (now_ns() - t0) / 1e9;
    emit("pageflip_per_sec", flips / el, "flips/s");
    if (flips)
        emit("pageflip_latency_us", el * 1e6 / flips, "us/flip");
}

// 2. Dumb-buffer lifecycle: create + map + unmap + destroy. Exercises graphics
//    memory allocation, which on a software-KMS kernel is plain page allocation
//    and on Linux goes through the GEM shmem path.
static void bench_dumb(uint32_t w, uint32_t h) {
    uint64_t t0 = now_ns(), deadline = t0 + (uint64_t)(g_secs * 1e9);
    uint64_t n = 0;
    while (now_ns() < deadline) {
        struct fb f;
        memset(&f, 0, sizeof f);
        if (make_fb(&f, w, h, 1) < 0) {
            free_fb(&f);
            skip("dumb_cycle_per_sec", strerror(errno));
            return;
        }
        free_fb(&f);
        n++;
    }
    double el = (now_ns() - t0) / 1e9;
    emit("dumb_cycle_per_sec", n / el, "cycles/s");
}

// 3. Framebuffer write bandwidth: the software-rasterizer path. Eclipse renders
//    with pixman into a mapped dumb buffer, so how fast the CPU can write
//    through that mapping bounds the whole desktop's frame rate. Writes a
//    varying pattern (not memset of a constant) so nothing can be optimized
//    into a no-op, and touches every pixel.
static void bench_fbwrite(struct fb *f, uint32_t w, uint32_t h) {
    if (!f->map) {
        skip("fb_write_MiB_per_sec", "dumb buffer could not be mmapped");
        return;
    }
    uint64_t bytes = (uint64_t)f->pitch * h;
    uint64_t t0 = now_ns(), deadline = t0 + (uint64_t)(g_secs * 1e9);
    uint64_t total = 0;
    uint32_t frame = 0;
    while (now_ns() < deadline) {
        volatile uint32_t *px = (volatile uint32_t *)f->map;
        uint32_t v = 0xff000000u | (frame * 0x00010203u);
        uint64_t words = bytes / 4;
        for (uint64_t i = 0; i < words; i++)
            px[i] = v + (uint32_t)i;
        total += bytes;
        frame++;
    }
    double el = (now_ns() - t0) / 1e9;
    emit("fb_write_MiB_per_sec", (double)total / (1024.0 * 1024.0) / el, "MiB/s");
    if (frame)
        emit("fb_fullscreen_fill_per_sec", frame / el, "fills/s");
    (void)w;
}

// 4. vblank wait: presentation timing. The interval is the refresh period; the
//    spread across samples is the jitter a compositor would see as stutter.
static void bench_vblank(void) {
    union drm_wait_vblank_request vb;
    memset(&vb, 0, sizeof vb);
    vb.request.type = DRM_VBLANK_RELATIVE;
    vb.request.sequence = 1;
    if (xioctl(g_fd, DRM_IOCTL_WAIT_VBLANK, &vb) < 0) {
        skip("vblank_interval_ms", strerror(errno));
        return;
    }
    enum { N = 60 };
    double iv[N];
    uint64_t prev = now_ns();
    int got = 0;
    for (int i = 0; i < N; i++) {
        memset(&vb, 0, sizeof vb);
        vb.request.type = DRM_VBLANK_RELATIVE;
        vb.request.sequence = 1;
        if (xioctl(g_fd, DRM_IOCTL_WAIT_VBLANK, &vb) < 0)
            break;
        uint64_t t = now_ns();
        iv[got++] = (t - prev) / 1e6;
        prev = t;
    }
    if (got < 2) {
        skip("vblank_interval_ms", "too few samples");
        return;
    }
    double sum = 0, lo = iv[0], hi = iv[0];
    for (int i = 0; i < got; i++) {
        sum += iv[i];
        if (iv[i] < lo) lo = iv[i];
        if (iv[i] > hi) hi = iv[i];
    }
    double mean = sum / got, var = 0;
    for (int i = 0; i < got; i++)
        var += (iv[i] - mean) * (iv[i] - mean);
    emit("vblank_interval_ms", mean, "ms");
    emit("vblank_jitter_stddev_ms", approx_sqrt(var / got), "ms");
    emit("vblank_jitter_range_ms", hi - lo, "ms");
}

// 5. Cursor updates: the hardware-cursor path, which Eclipse composites in the
//    kernel. A slow cursor is the most visible kind of desktop lag.
static void bench_cursor(void) {
    struct fb cur;
    memset(&cur, 0, sizeof cur);
    struct drm_mode_create_dumb c;
    memset(&c, 0, sizeof c);
    c.width = 64;
    c.height = 64;
    c.bpp = 32;
    if (xioctl(g_fd, DRM_IOCTL_MODE_CREATE_DUMB, &c) < 0) {
        skip("cursor_move_per_sec", strerror(errno));
        return;
    }
    cur.handle = c.handle;
    cur.size = c.size;

    struct drm_mode_cursor cm;
    memset(&cm, 0, sizeof cm);
    cm.flags = 0x01 /* BO */;
    cm.crtc_id = g_crtc_id;
    cm.width = 64;
    cm.height = 64;
    cm.handle = c.handle;
    if (xioctl(g_fd, DRM_IOCTL_MODE_CURSOR, &cm) < 0) {
        skip("cursor_move_per_sec", strerror(errno));
        free_fb(&cur);
        return;
    }

    uint64_t t0 = now_ns(), deadline = t0 + (uint64_t)(g_secs * 1e9);
    uint64_t n = 0;
    while (now_ns() < deadline) {
        memset(&cm, 0, sizeof cm);
        cm.flags = 0x02 /* MOVE */;
        cm.crtc_id = g_crtc_id;
        cm.x = (int32_t)(n % 512);
        cm.y = (int32_t)((n / 512) % 512);
        if (xioctl(g_fd, DRM_IOCTL_MODE_CURSOR, &cm) < 0)
            break;
        n++;
    }
    double el = (now_ns() - t0) / 1e9;
    emit("cursor_move_per_sec", n / el, "moves/s");
    free_fb(&cur);
}

// 6. Atomic commits — the modern KMS path a current compositor uses.
//
// Measured with TEST_ONLY so what is timed is the atomic machinery itself
// (property lookup, state duplication, the driver's atomic_check) without
// changing scanout, which keeps the number comparable between two kernels whose
// actual flip cost differs. A full property enumeration is needed to build even
// a minimal commit, so this also doubles as a check that the property ABI works.

// Look up a property id by name on an object. Returns 0 if absent.
static uint32_t find_prop_id(uint32_t obj_id, uint32_t obj_type, const char *name) {
    struct drm_mode_obj_get_properties op;
    memset(&op, 0, sizeof op);
    op.obj_id = obj_id;
    op.obj_type = obj_type;
    if (xioctl(g_fd, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &op) < 0 || !op.count_props)
        return 0;
    if (op.count_props > 128)
        op.count_props = 128;
    uint32_t ids[128];
    uint64_t vals[128];
    op.props_ptr = (uint64_t)(uintptr_t)ids;
    op.prop_values_ptr = (uint64_t)(uintptr_t)vals;
    if (xioctl(g_fd, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &op) < 0)
        return 0;
    for (uint32_t i = 0; i < op.count_props; i++) {
        struct drm_mode_get_property gp;
        memset(&gp, 0, sizeof gp);
        gp.prop_id = ids[i];
        if (xioctl(g_fd, DRM_IOCTL_MODE_GETPROPERTY, &gp) < 0)
            continue;
        gp.name[sizeof gp.name - 1] = 0;
        if (strcmp(gp.name, name) == 0)
            return ids[i];
    }
    return 0;
}

// The primary plane driving our CRTC (the one already bound to it, else the
// first plane that lists it as possible).
static uint32_t find_plane_for_crtc(uint32_t crtc_id, uint32_t *crtc_index_out) {
    struct drm_mode_card_res res;
    memset(&res, 0, sizeof res);
    if (xioctl(g_fd, DRM_IOCTL_MODE_GETRESOURCES, &res) < 0)
        return 0;
    uint32_t crtcs[32];
    if (res.count_crtcs > 32)
        res.count_crtcs = 32;
    res.crtc_id_ptr = (uint64_t)(uintptr_t)crtcs;
    res.count_connectors = res.count_encoders = res.count_fbs = 0;
    res.connector_id_ptr = res.encoder_id_ptr = res.fb_id_ptr = 0;
    if (xioctl(g_fd, DRM_IOCTL_MODE_GETRESOURCES, &res) < 0)
        return 0;
    uint32_t idx = 0;
    for (uint32_t i = 0; i < res.count_crtcs; i++)
        if (crtcs[i] == crtc_id)
            idx = i;
    *crtc_index_out = idx;

    struct drm_mode_get_plane_res pr;
    memset(&pr, 0, sizeof pr);
    if (xioctl(g_fd, DRM_IOCTL_MODE_GETPLANERESOURCES, &pr) < 0 || !pr.count_planes)
        return 0;
    if (pr.count_planes > 64)
        pr.count_planes = 64;
    uint32_t planes[64];
    pr.plane_id_ptr = (uint64_t)(uintptr_t)planes;
    if (xioctl(g_fd, DRM_IOCTL_MODE_GETPLANERESOURCES, &pr) < 0)
        return 0;

    uint32_t fallback = 0;
    for (uint32_t i = 0; i < pr.count_planes; i++) {
        struct drm_mode_get_plane gp;
        memset(&gp, 0, sizeof gp);
        gp.plane_id = planes[i];
        if (xioctl(g_fd, DRM_IOCTL_MODE_GETPLANE, &gp) < 0)
            continue;
        if (gp.crtc_id == crtc_id)
            return planes[i];
        if (!fallback && (gp.possible_crtcs & (1u << idx)))
            fallback = planes[i];
    }
    return fallback;
}

static void bench_atomic(struct fb *f, uint32_t w, uint32_t h) {
    struct drm_set_client_cap cap;
    memset(&cap, 0, sizeof cap);
    cap.capability = DRM_CLIENT_CAP_ATOMIC;
    cap.value = 1;
    if (xioctl(g_fd, DRM_IOCTL_SET_CLIENT_CAP, &cap) < 0) {
        skip("atomic_commit_per_sec", "atomic client cap rejected");
        return;
    }
    printf("atomic_supported=yes\n");
    fflush(stdout);

    uint32_t crtc_idx = 0;
    uint32_t plane = find_plane_for_crtc(g_crtc_id, &crtc_idx);
    if (!plane) {
        skip("atomic_commit_per_sec", "no usable plane for the CRTC");
        return;
    }

    // The minimal set a plane update needs.
    static const char *names[] = {"FB_ID",  "CRTC_ID", "SRC_X",  "SRC_Y",
                                  "SRC_W",  "SRC_H",   "CRTC_X", "CRTC_Y",
                                  "CRTC_W", "CRTC_H"};
    enum { NPROP = 10 };
    uint32_t pids[NPROP];
    uint64_t pvals[NPROP];
    for (int i = 0; i < NPROP; i++) {
        pids[i] = find_prop_id(plane, DRM_MODE_OBJECT_PLANE, names[i]);
        if (!pids[i]) {
            printf("atomic_commit_per_sec=unsupported (plane property %s missing)\n",
                   names[i]);
            fflush(stdout);
            return;
        }
    }
    // SRC_* are 16.16 fixed point; CRTC_* are plain pixels.
    pvals[0] = f->fb_id;
    pvals[1] = g_crtc_id;
    pvals[2] = 0;
    pvals[3] = 0;
    pvals[4] = (uint64_t)w << 16;
    pvals[5] = (uint64_t)h << 16;
    pvals[6] = 0;
    pvals[7] = 0;
    pvals[8] = w;
    pvals[9] = h;

    uint32_t objs[1] = {plane};
    uint32_t counts[1] = {NPROP};

    struct drm_mode_atomic at;
    memset(&at, 0, sizeof at);
    at.flags = DRM_MODE_ATOMIC_TEST_ONLY;
    at.count_objs = 1;
    at.objs_ptr = (uint64_t)(uintptr_t)objs;
    at.count_props_ptr = (uint64_t)(uintptr_t)counts;
    at.props_ptr = (uint64_t)(uintptr_t)pids;
    at.prop_values_ptr = (uint64_t)(uintptr_t)pvals;

    if (xioctl(g_fd, DRM_IOCTL_MODE_ATOMIC, &at) < 0) {
        skip("atomic_commit_per_sec", strerror(errno));
        return;
    }

    uint64_t t0 = now_ns(), deadline = t0 + (uint64_t)(g_secs * 1e9);
    uint64_t n = 0;
    while (now_ns() < deadline) {
        if (xioctl(g_fd, DRM_IOCTL_MODE_ATOMIC, &at) < 0)
            break;
        n++;
    }
    double el = (now_ns() - t0) / 1e9;
    emit("atomic_commit_per_sec", n / el, "commits/s");
    if (n)
        emit("atomic_commit_latency_us", el * 1e6 / n, "us/commit");
}

int main(int argc, char **argv) {
    const char *dev = argc > 1 ? argv[1] : "/dev/dri/card0";
    if (argc > 2)
        g_secs = atof(argv[2]);
    if (g_secs <= 0)
        g_secs = 3.0;

    g_fd = open(dev, O_RDWR | O_CLOEXEC);
    if (g_fd < 0) {
        fprintf(stderr, "drmbench: open %s: %s\n", dev, strerror(errno));
        return 1;
    }
    // Best-effort: without master, modeset ioctls are refused. Not fatal — a
    // kernel that hands them out anyway still benchmarks fine.
    xioctl(g_fd, DRM_IOCTL_SET_MASTER, NULL);

    struct drm_set_client_cap cap;
    memset(&cap, 0, sizeof cap);
    cap.capability = DRM_CLIENT_CAP_UNIVERSAL_PLANES;
    cap.value = 1;
    xioctl(g_fd, DRM_IOCTL_SET_CLIENT_CAP, &cap);

    if (setup_mode() < 0) {
        fprintf(stderr, "drmbench: no connected connector with a mode on %s\n", dev);
        return 1;
    }
    uint32_t w = g_mode.hdisplay, h = g_mode.vdisplay;
    fprintf(stderr, "drmbench: %s crtc=%u conn=%u mode=%ux%u@%u secs=%.1f\n",
            dev, g_crtc_id, g_conn_id, w, h, g_mode.vrefresh, g_secs);
    printf("mode_width=%u\nmode_height=%u\nmode_refresh=%u\n", w, h, g_mode.vrefresh);
    fflush(stdout);

    struct fb a, b;
    memset(&a, 0, sizeof a);
    memset(&b, 0, sizeof b);
    if (make_fb(&a, w, h, 1) < 0 || make_fb(&b, w, h, 1) < 0) {
        fprintf(stderr, "drmbench: framebuffer setup failed: %s\n", strerror(errno));
        free_fb(&a);
        free_fb(&b);
        return 1;
    }
    if (set_crtc(a.fb_id) < 0)
        fprintf(stderr, "drmbench: SETCRTC failed (%s) — flips may be refused\n",
                strerror(errno));

    bench_fbwrite(&a, w, h);
    bench_pageflip(&a, &b);
    bench_vblank();
    bench_cursor();
    bench_atomic(&a, w, h);
    free_fb(&a);
    free_fb(&b);
    bench_dumb(w, h);

    close(g_fd);
    return 0;
}
