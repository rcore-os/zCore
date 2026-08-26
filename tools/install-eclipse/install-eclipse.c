#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <unistd.h>
#include <fcntl.h>
#include <stdint.h>
#include <inttypes.h>
#include <stdarg.h>
#include <time.h>
#include <dirent.h>
#include <sys/stat.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <sys/mount.h>
#include <linux/fs.h>
#include <errno.h>
#include <zlib.h>

#define SECTOR_SIZE 512ULL
#define PART1_START 2048ULL
#define PART1_SIZE_MIB 1024ULL
#define PART1_SECTORS ((PART1_SIZE_MIB * 1024ULL * 1024ULL) / SECTOR_SIZE)
#define GPT_BACKUP_RESERVED_SECTORS 34ULL
#define MAX_DISKS 64
#define LINE_BUF 4096
#define SHA256_HEX_LEN 64
#define VERIFY_PREFIX_BYTES (16U * 4096U)
#define GIB (1024ULL * 1024ULL * 1024ULL)
#define MIB (1024ULL * 1024ULL)
#define PART1_BYTES ((uint64_t)PART1_SIZE_MIB * MIB)

static const char *resolve_image_path(const char *path);

#define UPD_STAGING_IMG "/tmp/eclipse-upd-efi.img"
#define UPD_STAGING_MNT "/tmp/eclipse-upd-staging"
#define UPD_EFI_MNT     "/tmp/eclipse-upd-efi"
#define UPD_ROOT_MNT    "/tmp/eclipse-upd-root"
#define EFI_IMAGE_GZ    resolve_image_path("/boot/efi.img.gz")
#define ROOTFS_IMAGE_GZ resolve_image_path("/boot/rootfs.btrfs.gz")
#define HOME_IMAGE_GZ   resolve_image_path("/boot/home.btrfs.gz")

#define INST_ROOT_MNT   "/tmp/eclipse-inst-root"

#define FSTAB_PLACEHOLDER_ROOT "__ECLIPSE_ROOT_DEV__"
#define FSTAB_PLACEHOLDER_EFI  "__ECLIPSE_EFI_DEV___"
#define FSTAB_PLACEHOLDER_HOME "__ECLIPSE_HOME_DEV__"
#define FSTAB_PLACEHOLDER_SWAP "__ECLIPSE_SWAP_DEV__"
#define FSTAB_TEMPLATE_LINE_ROOT "__ECLIPSE_ROOT_DEV__  /                  btrfs   defaults          0  1"
#define FSTAB_TEMPLATE_LINE_EFI  "__ECLIPSE_EFI_DEV___  /boot/efi          vfat    defaults,noatime  0  0"
#define FSTAB_TEMPLATE_LINE_HOME "__ECLIPSE_HOME_DEV__  /home              btrfs   defaults          0  0"
#define FSTAB_TEMPLATE_LINE_SWAP "__ECLIPSE_SWAP_DEV__  none               swap    sw                0  0"
/* Placeholder de la cmdline de rboot.conf (clave ROOT=). Debe medir
 * exactamente FSTAB_PLACEHOLDER_LEN (20) caracteres y ser distinto de los de
 * fstab para no colisionar con la imagen initramfs alojada en la partición EFI. */
#define RBOOT_PLACEHOLDER_ROOT "__ECLIPSE_CMDROOTDEV"
#define RBOOT_TEMPLATE_ROOT_KEY "ROOT=__ECLIPSE_CMDROOTDEV"
#define FSTAB_PLACEHOLDER_LEN  20U
#define FSTAB_PATCH_CHUNK      (64U * 1024U)
#define FSTAB_PATCH_MAX_SCAN   (160U * 1024U * 1024U)
#define FSTAB_PATCH_OVERLAP    FSTAB_PLACEHOLDER_LEN

#define GPT_ESP_TYPE   "C12A7328-F81F-11D2-BA4B-00A0C93EC93B"
#define GPT_LINUX_TYPE "0FC63DAF-8483-4772-8E79-3D69D8477DE4"

// ANSI escape codes for styling
#define COLOR_RESET   "\x1b[0m"
#define COLOR_BOLD    "\x1b[1m"
#define COLOR_RED     "\x1b[31m"
#define COLOR_GREEN   "\x1b[32m"
#define COLOR_YELLOW  "\x1b[33m"
#define COLOR_BLUE    "\x1b[34m"
#define COLOR_CYAN    "\x1b[36m"
#define COLOR_WHITE   "\x1b[37m"

#define BG_BLUE       "\x1b[44m"

#define log installer_log

enum install_mode {
    MODE_UNSET = 0,
    MODE_NEW,
    MODE_UPGRADE
};

enum layout_mode {
    LAYOUT_UNSET = 0,
    LAYOUT_SIMPLE,
    LAYOUT_ADVANCED
};

enum table_mode {
    TABLE_UNSET = 0,
    TABLE_MBR,
    TABLE_GPT
};

struct partition_entry {
    uint8_t boot_indicator;
    uint8_t starting_chs[3];
    uint8_t type;
    uint8_t ending_chs[3];
    uint32_t starting_lba;
    uint32_t sectors_count;
} __attribute__((packed));

struct disk_info {
    char path[128];
    uint64_t size_bytes;
    int removable;
};

struct existing_install_info {
    int has_valid_mbr;
    int has_efi_partition;
    int has_gpt_header;
    int suggest_upgrade;
};

struct config {
    char disk_path[128];
    int disk_set;
    enum install_mode mode;
    int mode_set;
    int auto_yes;
    int dry_run;
    enum layout_mode layout;
    int layout_set;
    enum table_mode table;
    int table_set;
};

struct partition_plan {
    char name[16];
    uint64_t start_sector;
    uint64_t sectors;
    uint8_t mbr_type;
    const char *gpt_type;
    const char *image_path;
    int write_image;
    int verify_after_write;
};

struct discovered_partitions {
    int efi_part_num;
    int root_part_num;
};

static FILE *g_log_file = NULL;

int struct_file_exists(const char *path);
static void installer_log(const char *fmt, ...);
static void clear_screen(void);
static void print_header(void);
static uint64_t get_disk_size_bytes(const char *path);
static uint64_t get_disk_size_raw_sysfs(const char *path);
static int scan_disks(struct disk_info disks[MAX_DISKS], char disk_list[MAX_DISKS][128]);
static int add_disk_if_present(struct disk_info disks[MAX_DISKS], char disk_list[MAX_DISKS][128], int found_disks, const char *path);
static int scan_disks_sysfs(struct disk_info disks[MAX_DISKS], char disk_list[MAX_DISKS][128], int found_disks);
static void trim_newline(char *s);
static void trim_leading_ws(char *s);
static int prompt_tty_fd(void);
static int safe_flush_input(char *buf, size_t size);
static int parse_menu_index(const char *input, int max_value, int *out_index);
static uint64_t align_up_u64(uint64_t value, uint64_t alignment);
static uint64_t align_down_u64(uint64_t value, uint64_t alignment);
static void format_size(char *buf, size_t buf_size, uint64_t size_bytes);
static const char *mode_label(enum install_mode mode);
static const char *layout_label(enum layout_mode layout);
static const char *table_label(enum table_mode table);
static const char *disk_basename(const char *disk_path);
static int is_removable_disk(const char *disk_path);
static int detect_existing_install(const char *disk_path, struct existing_install_info *info);
static int parse_args(int argc, char **argv, struct config *cfg);
static int prompt_disk_selection(struct config *cfg, struct disk_info disks[MAX_DISKS], char disk_list[MAX_DISKS][128], int found_disks);
static int prompt_install_mode(struct config *cfg, const struct existing_install_info *info);
static int prompt_layout(struct config *cfg);
static int prompt_table(struct config *cfg, int running_on_efi);
static int ask_yes_no(const char *prompt, int default_yes, int auto_yes);
static int command_exists(const char *name);
static int run_command_logged(const char *cmd, int dry_run);
static int run_command_capture_token(const char *cmd, char *out, size_t out_size);
static int verify_sha256_file(const char *image_path);

typedef struct {
    uint32_t state[8];
    uint64_t bitcount;
    uint8_t buffer[64];
} sha256_ctx;

static const uint32_t sha256_k[64] = {
    0x428a2f98U, 0x71374491U, 0xb5c0fbcfU, 0xe9b5dba5U, 0x3956c25bU, 0x59f111f1U, 0x923f82a4U, 0xab1c5ed5U,
    0xd807aa98U, 0x12835b01U, 0x243185beU, 0x550c7dc3U, 0x72be5d74U, 0x80deb1feU, 0x9bdc06a7U, 0xc19bf174U,
    0xe49b69c1U, 0xefbe4786U, 0x0fc19dc6U, 0x240ca1ccU, 0x2de92c6fU, 0x4a7484aaU, 0x5cb0a9dcU, 0x76f988daU,
    0x983e5152U, 0xa831c66dU, 0xb00327c8U, 0xbf597fc7U, 0xc6e00bf3U, 0xd5a79147U, 0x06ca6351U, 0x14292967U,
    0x27b70a85U, 0x2e1b2138U, 0x4d2c6dfcU, 0x53380d13U, 0x650a7354U, 0x766a0abbU, 0x81c2c92eU, 0x92722c85U,
    0xa2bfe8a1U, 0xa81a664bU, 0xc24b8b70U, 0xc76c51a3U, 0xd192e819U, 0xd6990624U, 0xf40e3585U, 0x106aa070U,
    0x19a4c116U, 0x1e376c08U, 0x2748774cU, 0x34b0bcb5U, 0x391c0cb3U, 0x4ed8aa4aU, 0x5b9cca4fU, 0x682e6ff3U,
    0x748f82eeU, 0x78a5636fU, 0x84c87814U, 0x8cc70208U, 0x90befffaU, 0xa4506cebU, 0xbef9a3f7U, 0xc67178f2U
};

static uint32_t sha256_rotr(uint32_t x, uint32_t n) {
    return (x >> n) | (x << (32U - n));
}

static void sha256_init(sha256_ctx *ctx) {
    ctx->state[0] = 0x6a09e667U;
    ctx->state[1] = 0xbb67ae85U;
    ctx->state[2] = 0x3c6ef372U;
    ctx->state[3] = 0xa54ff53aU;
    ctx->state[4] = 0x510e527fU;
    ctx->state[5] = 0x9b05688cU;
    ctx->state[6] = 0x1f83d9abU;
    ctx->state[7] = 0x5be0cd19U;
    ctx->bitcount = 0;
    memset(ctx->buffer, 0, sizeof(ctx->buffer));
}

static void sha256_transform(sha256_ctx *ctx, const uint8_t block[64]) {
    uint32_t w[64];
    uint32_t a, b, c, d, e, f, g, h;
    int i;

    for (i = 0; i < 16; i++) {
        w[i] = ((uint32_t)block[i * 4] << 24) |
               ((uint32_t)block[i * 4 + 1] << 16) |
               ((uint32_t)block[i * 4 + 2] << 8) |
               ((uint32_t)block[i * 4 + 3]);
    }
    for (i = 16; i < 64; i++) {
        uint32_t s0 = sha256_rotr(w[i - 15], 7) ^ sha256_rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
        uint32_t s1 = sha256_rotr(w[i - 2], 17) ^ sha256_rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }

    a = ctx->state[0];
    b = ctx->state[1];
    c = ctx->state[2];
    d = ctx->state[3];
    e = ctx->state[4];
    f = ctx->state[5];
    g = ctx->state[6];
    h = ctx->state[7];

    for (i = 0; i < 64; i++) {
        uint32_t s1 = sha256_rotr(e, 6) ^ sha256_rotr(e, 11) ^ sha256_rotr(e, 25);
        uint32_t ch = (e & f) ^ ((~e) & g);
        uint32_t temp1 = h + s1 + ch + sha256_k[i] + w[i];
        uint32_t s0 = sha256_rotr(a, 2) ^ sha256_rotr(a, 13) ^ sha256_rotr(a, 22);
        uint32_t maj = (a & b) ^ (a & c) ^ (b & c);
        uint32_t temp2 = s0 + maj;

        h = g;
        g = f;
        f = e;
        e = d + temp1;
        d = c;
        c = b;
        b = a;
        a = temp1 + temp2;
    }

    ctx->state[0] += a;
    ctx->state[1] += b;
    ctx->state[2] += c;
    ctx->state[3] += d;
    ctx->state[4] += e;
    ctx->state[5] += f;
    ctx->state[6] += g;
    ctx->state[7] += h;
}

static void sha256_update(sha256_ctx *ctx, const uint8_t *data, size_t len) {
    size_t i = 0;

    while (i < len) {
        size_t fill = (size_t)(ctx->bitcount / 8 % 64);
        size_t left = 64 - fill;
        size_t chunk = len - i;

        if (chunk >= left) {
            memcpy(ctx->buffer + fill, data + i, left);
            ctx->bitcount += (uint64_t)left * 8U;
            sha256_transform(ctx, ctx->buffer);
            i += left;
        } else {
            memcpy(ctx->buffer + fill, data + i, chunk);
            ctx->bitcount += (uint64_t)chunk * 8U;
            break;
        }
    }
}

static void sha256_final(sha256_ctx *ctx, uint8_t digest[32]) {
    uint8_t pad[64];
    size_t fill = (size_t)((ctx->bitcount / 8) % 64);
    size_t padlen = (fill < 56) ? (56 - fill) : (64 + 56 - fill);
    uint64_t bits = ctx->bitcount;
    uint8_t len_be[8];
    int i;

    memset(pad, 0, sizeof(pad));
    pad[0] = 0x80;
    sha256_update(ctx, pad, padlen);
    for (i = 7; i >= 0; i--) {
        len_be[i] = (uint8_t)(bits & 0xffU);
        bits >>= 8;
    }
    sha256_update(ctx, len_be, sizeof(len_be));

    for (i = 0; i < 8; i++) {
        digest[i * 4] = (uint8_t)(ctx->state[i] >> 24);
        digest[i * 4 + 1] = (uint8_t)(ctx->state[i] >> 16);
        digest[i * 4 + 2] = (uint8_t)(ctx->state[i] >> 8);
        digest[i * 4 + 3] = (uint8_t)(ctx->state[i]);
    }
}

static void sha256_hex_digest(const uint8_t digest[32], char *out) {
    static const char hex[] = "0123456789abcdef";
    int i;

    for (i = 0; i < 32; i++) {
        out[i * 2] = hex[digest[i] >> 4];
        out[i * 2 + 1] = hex[digest[i] & 0x0f];
    }
    out[64] = 0;
}

static void sha256_hex_buffer(const uint8_t *data, size_t len, char *out) {
    sha256_ctx ctx;
    uint8_t digest[32];

    sha256_init(&ctx);
    sha256_update(&ctx, data, len);
    sha256_final(&ctx, digest);
    sha256_hex_digest(digest, out);
}

static ssize_t disk_pread_all(int fd, void *buf, size_t len, uint64_t offset) {
    uint8_t *p = buf;
    size_t left = len;

    while (left > 0) {
        ssize_t n = pread(fd, p, left, (off_t)offset);
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            return -1;
        }
        if (n == 0) {
            break;
        }
        p += (size_t)n;
        offset += (uint64_t)n;
        left -= (size_t)n;
    }
    return (ssize_t)(len - left);
}

static int sha256_file_path(const char *path, char *hex_out) {
    uint8_t buf[4096];
    sha256_ctx ctx;
    uint8_t digest[32];
    FILE *fp;
    size_t nread;

    fp = fopen(path, "rb");
    if (fp == NULL) {
        return -1;
    }

    sha256_init(&ctx);
    while ((nread = fread(buf, 1, sizeof(buf), fp)) > 0) {
        sha256_update(&ctx, buf, nread);
    }
    if (ferror(fp)) {
        fclose(fp);
        return -1;
    }
    fclose(fp);

    sha256_final(&ctx, digest);
    sha256_hex_digest(digest, hex_out);
    return 0;
}

static int read_gz_prefix(const char *gz_path, uint8_t *buf, size_t len) {
    gzFile gz;
    size_t total = 0;

    gz = gzopen(gz_path, "rb");
    if (gz == NULL) {
        return -1;
    }

    while (total < len) {
        int nread = gzread(gz, buf + total, (unsigned)(len - total));
        if (nread < 0) {
            gzclose(gz);
            return -1;
        }
        if (nread == 0) {
            break;
        }
        total += (size_t)nread;
    }

    if (gzclose(gz) != Z_OK) {
        return -1;
    }
    return (total == len) ? 0 : -1;
}

/* ISIZE del trailer gzip (tamaño descomprimido mod 2^32). */
static int gzip_uncompressed_size(const char *gz_path, uint64_t *out_size) {
    FILE *fp;
    unsigned char buf[4];

    if (out_size == NULL) {
        return -1;
    }
    fp = fopen(gz_path, "rb");
    if (fp == NULL) {
        return -1;
    }
    if (fseek(fp, -4, SEEK_END) != 0) {
        fclose(fp);
        return -1;
    }
    if (fread(buf, 1, 4, fp) != 4) {
        fclose(fp);
        return -1;
    }
    fclose(fp);
    *out_size = (uint64_t)buf[0] | ((uint64_t)buf[1] << 8) | ((uint64_t)buf[2] << 16) |
                ((uint64_t)buf[3] << 24);
    return 0;
}

static int verify_efi_image_fits_partition(const struct partition_plan *efi_part) {
    uint64_t unc;
    uint64_t part_bytes;

    if (efi_part == NULL || efi_part->image_path == NULL) {
        return 0;
    }
    part_bytes = efi_part->sectors * SECTOR_SIZE;
    if (gzip_uncompressed_size(efi_part->image_path, &unc) != 0) {
        log(COLOR_YELLOW
            "ADVERTENCIA: No se pudo leer el tamaño de %s; se omite comprobación de capacidad EFI."
            COLOR_RESET,
            efi_part->image_path);
        return 0;
    }
    if (unc > part_bytes) {
        log(COLOR_RED COLOR_BOLD
            "ERROR: efi.img.gz descomprimido (%llu MiB) excede la partición EFI (%llu MiB)."
            COLOR_RESET,
            (unsigned long long)(unc / MIB), (unsigned long long)(part_bytes / MIB));
        log("Reconstruya la imagen live (`make image`) con efi.img del mismo tamaño o aumente PART1_SIZE_MIB.");
        return -1;
    }
    return 0;
}

static int build_partition_plan(enum layout_mode layout, enum table_mode table, uint64_t disk_sectors,
                                struct partition_plan *parts, int *part_count,
                                char *summary, size_t summary_size);
static int prepare_gpt_or_fallback(enum table_mode *table);
static int write_mbr_partition_table(const char *disk_path, const struct partition_plan *parts, int part_count, int dry_run);
static int write_gpt_partition_table_native(const char *disk_path, const struct partition_plan *parts, int part_count);
static int write_gpt_partition_table(const char *disk_path, const struct partition_plan *parts, int part_count, int dry_run);
static int write_partition_image_with_retry(const char *part_path, const struct partition_plan *part, int dry_run);
static ssize_t disk_pwrite_all(int fd, const void *buf, size_t len, uint64_t offset);
static int decompress_gz_to_path(const char *gz_path, const char *out_path, int dry_run);
static int write_gz_image_to_disk(const char *gz_path, const char *disk_path, uint64_t start_sector, int dry_run);
static int verify_partition_write(const char *part_path, const struct partition_plan *part, int dry_run);
static int partition_dev_path(const char *disk_path, int part_num, char *out, size_t out_size);
static int discover_partitions(const char *disk_path, struct discovered_partitions *out);
static int shell_mkdir_p(const char *path, int dry_run);
static int shell_copy_file(const char *src, const char *dst, int dry_run);
static int shell_mount(const char *source, const char *target, const char *fstype, int loop, int dry_run);
static int shell_umount(const char *target, int dry_run);
static int copy_upgrade_payload_if_exists(const char *src, const char *dst, int dry_run);
static int run_upgrade(const char *disk_path, int dry_run);
static int write_fstab_to_root(const char *disk_path, const struct partition_plan *parts,
                               int part_count, enum layout_mode layout, int dry_run);
static int resize_root_to_partition(const char *root_dev, int dry_run);
static int patch_rboot_root_on_efi(const char *block_dev, const char *root_dev, int dry_run);
static void close_log_file(void);

static void close_log_file(void) {
    if (g_log_file != NULL) {
        fclose(g_log_file);
        g_log_file = NULL;
    }
}

static void installer_log(const char *fmt, ...) {
    char message[LINE_BUF];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(message, sizeof(message), fmt, ap);
    va_end(ap);

    fprintf(stdout, "%s\n", message);
    fflush(stdout);

    if (g_log_file != NULL) {
        time_t now = time(NULL);
        struct tm tm_now;
        localtime_r(&now, &tm_now);
        char ts[32];
        strftime(ts, sizeof(ts), "%Y-%m-%d %H:%M:%S", &tm_now);
        fprintf(g_log_file, "[%s] %s\n", ts, message);
        fflush(g_log_file);
    }
}

static void clear_screen(void) {
    fprintf(stdout, "\x1b[2J\x1b[H");
    fflush(stdout);
}

static void print_header(void) {
    clear_screen();
    log(COLOR_CYAN COLOR_BOLD "========================================================" COLOR_RESET);
    log(COLOR_BLUE COLOR_BOLD " *               ECLIPSE OS 2 INSTALLER               *" COLOR_RESET);
    log(COLOR_CYAN COLOR_BOLD "========================================================\n" COLOR_RESET);
}

static uint64_t read_u64_from_file(const char *path) {
    FILE *fp = fopen(path, "r");
    if (fp == NULL) {
        return 0;
    }

    unsigned long long value = 0;
    int scanned = fscanf(fp, "%llu", &value);
    fclose(fp);
    if (scanned != 1) {
        return 0;
    }
    return (uint64_t)value;
}

static uint64_t get_disk_size_raw_sysfs(const char *path) {
    char size_path[256];
    const char *base = disk_basename(path);
    snprintf(size_path, sizeof(size_path), "/sys/class/block/%s/size", base);
    uint64_t raw = read_u64_from_file(size_path);
    if (raw == 0) {
        return 0;
    }
    return raw;
}

static uint64_t get_disk_size_bytes(const char *path) {
    uint64_t sysfs_raw = get_disk_size_raw_sysfs(path);
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        if (sysfs_raw == 0) {
            return 0;
        }
        if (sysfs_raw > (UINT64_MAX / SECTOR_SIZE)) {
            return sysfs_raw;
        }
        return sysfs_raw * SECTOR_SIZE;
    }

    uint64_t size = 0;
    if (ioctl(fd, BLKGETSIZE64, &size) != 0 || size == 0) {
        struct stat st;
        if (fstat(fd, &st) == 0 && S_ISBLK(st.st_mode) && (uint64_t)st.st_size > 0) {
            size = (uint64_t)st.st_size;
        }
    }
    if (size == 0) {
        off_t end = lseek(fd, 0, SEEK_END);
        if (end > 0) {
            size = (uint64_t)end;
        }
    }

    close(fd);

    if (sysfs_raw == 0) {
        return size;
    }

    uint64_t sysfs_as_sectors = (sysfs_raw > (UINT64_MAX / SECTOR_SIZE)) ? 0 : (sysfs_raw * SECTOR_SIZE);
    uint64_t sysfs_size = sysfs_as_sectors;
    if (size > 0) {
        uint64_t sysfs_as_bytes = sysfs_raw;
        if (sysfs_as_bytes > 0) {
            uint64_t diff_bytes = (size > sysfs_as_bytes) ? (size - sysfs_as_bytes) : (sysfs_as_bytes - size);
            uint64_t diff_sectors = (size > sysfs_as_sectors) ? (size - sysfs_as_sectors) : (sysfs_as_sectors - size);
            if (diff_bytes < diff_sectors) {
                sysfs_size = sysfs_as_bytes;
            }
        }
    }

    if (size == 0) {
        return sysfs_size;
    }
    if (sysfs_size == 0) {
        return size;
    }

    uint64_t diff = (size > sysfs_size) ? (size - sysfs_size) : (sysfs_size - size);
    if (diff > (sysfs_size / 20U)) {
        return (size < sysfs_size) ? size : sysfs_size;
    }
    return size;
}

static void trim_newline(char *s) {
    if (s == NULL) {
        return;
    }
    s[strcspn(s, "\r\n")] = 0;
}

static void trim_leading_ws(char *s) {
    size_t skip;

    if (s == NULL) {
        return;
    }
    skip = strspn(s, " \t");
    if (skip > 0) {
        memmove(s, s + skip, strlen(s + skip) + 1);
    }
}

/// TTY de consola para prompts interactivos (evita stdin redirigido o sin eco).
static int prompt_tty_fd(void) {
    static int tty_fd = -2;

    if (tty_fd == -2) {
        tty_fd = open("/dev/tty", O_RDWR | O_CLOEXEC);
        if (tty_fd < 0) {
            tty_fd = STDIN_FILENO;
        }
    }
    return tty_fd;
}

/// Lee una línea desde /dev/tty byte a byte (sin depender del line discipline de musl).
static int safe_flush_input(char *buf, size_t size) {
    int fd;
    size_t pos;
    char c;

    if (buf == NULL || size < 2) {
        return -1;
    }

    fd = prompt_tty_fd();
    fflush(stdout);
    pos = 0;

    while (pos + 1 < size) {
        ssize_t n;

        do {
            n = read(fd, &c, 1);
        } while (n < 0 && errno == EINTR);

        if (n <= 0) {
            buf[pos] = '\0';
            return (pos == 0) ? -1 : 0;
        }
        if (c == '\r') {
            continue;
        }
        if (c == '\n') {
            break;
        }
        if (c == '\b' || c == 127) {
            if (pos > 0) {
                pos--;
            }
            continue;
        }
        buf[pos++] = c;
    }

    buf[pos] = '\0';
    trim_leading_ws(buf);
    return 0;
}

/// Parse a menu index like "1" or "2". Returns 1 on success, 0 for empty input
/// (use default), -1 if the value is not a valid index in [1, max_value].
static int parse_menu_index(const char *input, int max_value, int *out_index) {
    char *end;

    if (input == NULL || input[0] == '\0') {
        return 0;
    }

    {
        long value = strtol(input, &end, 10);
        if (*end != '\0' || value < 1 || value > max_value) {
            return -1;
        }
        *out_index = (int)value;
    }
    return 1;
}

static uint64_t align_up_u64(uint64_t value, uint64_t alignment) {
    if (alignment == 0) {
        return value;
    }
    uint64_t rem = value % alignment;
    if (rem == 0) {
        return value;
    }
    return value + (alignment - rem);
}

static uint64_t align_down_u64(uint64_t value, uint64_t alignment) {
    if (alignment == 0) {
        return value;
    }
    return value - (value % alignment);
}

static void format_size(char *buf, size_t buf_size, uint64_t size_bytes) {
    double gib = (double)size_bytes / (1024.0 * 1024.0 * 1024.0);
    double mib = (double)size_bytes / (1024.0 * 1024.0);
    if (gib >= 1.0) {
        snprintf(buf, buf_size, "%.2f GB", gib);
    } else {
        snprintf(buf, buf_size, "%.2f MB", mib);
    }
}

static const char *mode_label(enum install_mode mode) {
    switch (mode) {
        case MODE_NEW:
            return "Nueva instalación";
        case MODE_UPGRADE:
            return "Actualización";
        default:
            return "Sin definir";
    }
}

static void part1_size_label(char *buf, size_t len) {
    if (PART1_SIZE_MIB >= 1024 && (PART1_SIZE_MIB % 1024) == 0) {
        snprintf(buf, len, "%llu GB", (unsigned long long)(PART1_SIZE_MIB / 1024));
    } else {
        snprintf(buf, len, "%llu MB", (unsigned long long)PART1_SIZE_MIB);
    }
}

static const char *layout_label(enum layout_mode layout) {
    static char simple_buf[80];
    static char advanced_buf[96];
    char efi_label[16];

    part1_size_label(efi_label, sizeof(efi_label));
    switch (layout) {
        case LAYOUT_SIMPLE:
            snprintf(simple_buf, sizeof(simple_buf), "Simple (EFI %s + ROOT resto)", efi_label);
            return simple_buf;
        case LAYOUT_ADVANCED:
            snprintf(advanced_buf, sizeof(advanced_buf),
                      "Avanzado (EFI %s + ROOT + HOME + SWAP)", efi_label);
            return advanced_buf;
        default:
            return "Sin definir";
    }
}

static const char *table_label(enum table_mode table) {
    switch (table) {
        case TABLE_MBR:
            return "MBR/BIOS";
        case TABLE_GPT:
            return "GPT/UEFI";
        default:
            return "Sin cambios";
    }
}

static const char *disk_basename(const char *disk_path) {
    const char *slash = strrchr(disk_path, '/');
    return (slash != NULL) ? slash + 1 : disk_path;
}

static int is_removable_disk(const char *disk_path) {
    char sysfs_path[256];
    snprintf(sysfs_path, sizeof(sysfs_path), "/sys/block/%s/removable", disk_basename(disk_path));
    FILE *fp = fopen(sysfs_path, "r");
    if (fp == NULL) {
        return 0;
    }

    char buf[32];
    if (fgets(buf, sizeof(buf), fp) == NULL) {
        fclose(fp);
        return 0;
    }
    fclose(fp);
    trim_newline(buf);
    return strcmp(buf, "1") == 0;
}

static int detect_existing_install(const char *disk_path, struct existing_install_info *info) {
    memset(info, 0, sizeof(*info));

    int fd = open(disk_path, O_RDONLY);
    if (fd < 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo abrir %s para detectar instalaciones previas (%s)." COLOR_RESET,
            disk_path, strerror(errno));
        return -1;
    }

    unsigned char sector0[SECTOR_SIZE];
    ssize_t nread = pread(fd, sector0, sizeof(sector0), 0);
    if (nread != (ssize_t)sizeof(sector0)) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo leer el sector 0 de %s." COLOR_RESET, disk_path);
        close(fd);
        return -1;
    }

    info->has_valid_mbr = (sector0[510] == 0x55 && sector0[511] == 0xAA);
    info->has_efi_partition = (sector0[446 + 4] == 0xEF);

    unsigned char sector1[SECTOR_SIZE];
    nread = pread(fd, sector1, sizeof(sector1), (off_t)SECTOR_SIZE);
    if (nread == (ssize_t)sizeof(sector1) && memcmp(sector1, "EFI PART", 8) == 0) {
        info->has_gpt_header = 1;
    }

    info->suggest_upgrade = (info->has_valid_mbr && info->has_efi_partition) || info->has_gpt_header;
    close(fd);
    return 0;
}

static int add_disk_if_present(struct disk_info disks[MAX_DISKS], char disk_list[MAX_DISKS][128], int found_disks, const char *path) {
    if (found_disks >= MAX_DISKS) {
        return found_disks;
    }

    for (int i = 0; i < found_disks; i++) {
        if (strcmp(disks[i].path, path) == 0) {
            return found_disks;
        }
    }

    struct stat st;
    if (stat(path, &st) != 0 || !S_ISBLK(st.st_mode)) {
        return found_disks;
    }

    uint64_t size_bytes = get_disk_size_bytes(path);
    if (size_bytes == 0) {
        return found_disks;
    }
    char size_buf[64];
    format_size(size_buf, sizeof(size_buf), size_bytes);
    log("  [%d] " COLOR_YELLOW "%s" COLOR_RESET " (%s)", found_disks + 1, path, size_buf);

    strncpy(disks[found_disks].path, path, sizeof(disks[found_disks].path) - 1);
    disks[found_disks].path[sizeof(disks[found_disks].path) - 1] = 0;
    disks[found_disks].size_bytes = size_bytes;
    disks[found_disks].removable = is_removable_disk(path);

    strncpy(disk_list[found_disks], path, sizeof(disk_list[found_disks]) - 1);
    disk_list[found_disks][sizeof(disk_list[found_disks]) - 1] = 0;
    return found_disks + 1;
}

static int is_supported_disk_name(const char *name) {
    if (name == NULL || name[0] == '\0') {
        return 0;
    }
    return strncmp(name, "sd", 2) == 0 ||
           strncmp(name, "vd", 2) == 0 ||
           strncmp(name, "xvd", 3) == 0 ||
           strncmp(name, "hd", 2) == 0 ||
           strncmp(name, "nvme", 4) == 0 ||
           strncmp(name, "mmcblk", 6) == 0;
}

static int scan_disks_sysfs(struct disk_info disks[MAX_DISKS], char disk_list[MAX_DISKS][128], int found_disks) {
    DIR *dir = opendir("/sys/class/block");
    if (dir == NULL) {
        return found_disks;
    }

    struct dirent *ent;
    while ((ent = readdir(dir)) != NULL && found_disks < MAX_DISKS) {
        const char *name = ent->d_name;
        size_t name_len = strnlen(name, 128);
        if (strcmp(name, ".") == 0 || strcmp(name, "..") == 0) {
            continue;
        }
        if (name_len == 0 || name_len >= 96) {
            continue;
        }
        if (!is_supported_disk_name(name)) {
            continue;
        }

        char partition_path[256];
        snprintf(partition_path, sizeof(partition_path), "/sys/class/block/%.*s/partition", (int)name_len, name);
        if (access(partition_path, F_OK) == 0) {
            continue;
        }

        char dev_path[128];
        snprintf(dev_path, sizeof(dev_path), "/dev/%.*s", (int)name_len, name);
        found_disks = add_disk_if_present(disks, disk_list, found_disks, dev_path);
    }

    closedir(dir);
    return found_disks;
}

static int scan_disks(struct disk_info disks[MAX_DISKS], char disk_list[MAX_DISKS][128]) {
    int found_disks = scan_disks_sysfs(disks, disk_list, 0);

    for (char c = 'a'; c <= 'z' && found_disks < MAX_DISKS; c++) {
        char path[128];
        snprintf(path, sizeof(path), "/dev/sd%c", c);
        found_disks = add_disk_if_present(disks, disk_list, found_disks, path);
    }

    for (int ctrl = 0; ctrl <= 9 && found_disks < MAX_DISKS; ctrl++) {
        for (int ns = 1; ns <= 9 && found_disks < MAX_DISKS; ns++) {
            char path[128];
            snprintf(path, sizeof(path), "/dev/nvme%dn%d", ctrl, ns);
            found_disks = add_disk_if_present(disks, disk_list, found_disks, path);
        }
    }

    for (char c = 'a'; c <= 'z' && found_disks < MAX_DISKS; c++) {
        char path[128];
        snprintf(path, sizeof(path), "/dev/vd%c", c);
        found_disks = add_disk_if_present(disks, disk_list, found_disks, path);
    }

    return found_disks;
}

static int parse_args(int argc, char **argv, struct config *cfg) {
    memset(cfg, 0, sizeof(*cfg));
    cfg->mode = MODE_UNSET;
    cfg->layout = LAYOUT_UNSET;
    cfg->table = TABLE_UNSET;

    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--disk") == 0) {
            if (i + 1 >= argc) {
                log(COLOR_RED COLOR_BOLD "ERROR: --disk requiere una ruta." COLOR_RESET);
                return -1;
            }
            strncpy(cfg->disk_path, argv[++i], sizeof(cfg->disk_path) - 1);
            cfg->disk_path[sizeof(cfg->disk_path) - 1] = 0;
            cfg->disk_set = 1;
        } else if (strcmp(argv[i], "--mode") == 0) {
            if (i + 1 >= argc) {
                log(COLOR_RED COLOR_BOLD "ERROR: --mode requiere new o upgrade." COLOR_RESET);
                return -1;
            }
            const char *value = argv[++i];
            if (strcmp(value, "new") == 0) {
                cfg->mode = MODE_NEW;
            } else if (strcmp(value, "upgrade") == 0) {
                cfg->mode = MODE_UPGRADE;
            } else {
                log(COLOR_RED COLOR_BOLD "ERROR: modo inválido: %s" COLOR_RESET, value);
                return -1;
            }
            cfg->mode_set = 1;
        } else if (strcmp(argv[i], "--yes") == 0) {
            cfg->auto_yes = 1;
        } else if (strcmp(argv[i], "--dry-run") == 0) {
            cfg->dry_run = 1;
        } else if (strcmp(argv[i], "--simple") == 0) {
            if (cfg->layout_set && cfg->layout != LAYOUT_SIMPLE) {
                log(COLOR_RED COLOR_BOLD "ERROR: --simple y --advanced son mutuamente excluyentes." COLOR_RESET);
                return -1;
            }
            cfg->layout = LAYOUT_SIMPLE;
            cfg->layout_set = 1;
        } else if (strcmp(argv[i], "--advanced") == 0) {
            if (cfg->layout_set && cfg->layout != LAYOUT_ADVANCED) {
                log(COLOR_RED COLOR_BOLD "ERROR: --simple y --advanced son mutuamente excluyentes." COLOR_RESET);
                return -1;
            }
            cfg->layout = LAYOUT_ADVANCED;
            cfg->layout_set = 1;
        } else if (strcmp(argv[i], "--mbr") == 0) {
            if (cfg->table_set && cfg->table != TABLE_MBR) {
                log(COLOR_RED COLOR_BOLD "ERROR: --mbr y --gpt son mutuamente excluyentes." COLOR_RESET);
                return -1;
            }
            cfg->table = TABLE_MBR;
            cfg->table_set = 1;
        } else if (strcmp(argv[i], "--gpt") == 0) {
            if (cfg->table_set && cfg->table != TABLE_GPT) {
                log(COLOR_RED COLOR_BOLD "ERROR: --mbr y --gpt son mutuamente excluyentes." COLOR_RESET);
                return -1;
            }
            cfg->table = TABLE_GPT;
            cfg->table_set = 1;
        } else {
            log(COLOR_RED COLOR_BOLD "ERROR: argumento no reconocido: %s" COLOR_RESET, argv[i]);
            return -1;
        }
    }

    return 0;
}

static int prompt_disk_selection(struct config *cfg, struct disk_info disks[MAX_DISKS], char disk_list[MAX_DISKS][128], int found_disks) {
    if (cfg->disk_set) {
        log("Disco preseleccionado por CLI: " COLOR_YELLOW "%s" COLOR_RESET, cfg->disk_path);
        return 0;
    }

    if (found_disks <= 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se detectaron discos de almacenamiento." COLOR_RESET);
        return -1;
    }

    strncpy(cfg->disk_path, disk_list[0], sizeof(cfg->disk_path) - 1);
    cfg->disk_path[sizeof(cfg->disk_path) - 1] = 0;

    if (cfg->auto_yes) {
        log("--yes activo: usando el primer disco detectado (%s).", cfg->disk_path);
        return 0;
    }

    char input[64];
    log("Seleccione el disco de destino [por defecto: %s]:", cfg->disk_path);
    if (safe_flush_input(input, sizeof(input)) != 0) {
        log("Entrada vacía: usando %s.", cfg->disk_path);
        return 0;
    }

    if (input[0] == '\0') {
        return 0;
    }

    {
        int idx = 0;
        int parsed = parse_menu_index(input, found_disks, &idx);
        if (parsed == 1) {
            strncpy(cfg->disk_path, disk_list[idx - 1], sizeof(cfg->disk_path) - 1);
            cfg->disk_path[sizeof(cfg->disk_path) - 1] = 0;
            return 0;
        }
        if (parsed == -1 && strspn(input, "0123456789") == strlen(input)) {
            log(COLOR_RED "Opción numérica fuera de rango o inválida." COLOR_RESET);
            return 0;
        }
    }

    strncpy(cfg->disk_path, input, sizeof(cfg->disk_path) - 1);
    cfg->disk_path[sizeof(cfg->disk_path) - 1] = 0;
    (void)disks;
    return 0;
}

static int prompt_install_mode(struct config *cfg, const struct existing_install_info *info) {
    if (cfg->mode_set) {
        log("Modo preseleccionado por CLI: %s", mode_label(cfg->mode));
        return 0;
    }

    enum install_mode suggested = info->suggest_upgrade ? MODE_UPGRADE : MODE_NEW;
    if (info->suggest_upgrade) {
        log(COLOR_YELLOW COLOR_BOLD "Se detectó una instalación existente compatible; se recomienda ACTUALIZACIÓN." COLOR_RESET);
    } else {
        log("No se detectó instalación previa conocida; se recomienda NUEVA instalación.");
    }

    if (cfg->auto_yes) {
        cfg->mode = suggested;
        log("--yes activo: usando modo sugerido (%s).", mode_label(cfg->mode));
        return 0;
    }

    char input[32];
    log("Seleccione el modo de instalación:");
    log("  [1] Nueva instalación (reparticiona y reescribe imágenes)");
    log("  [2] Actualización (Sin reparticionado ni reinstalacion de sistema.)");
    log("Opción recomendada [%d]:", suggested == MODE_NEW ? 1 : 2);
    if (safe_flush_input(input, sizeof(input)) != 0 || input[0] == '\0') {
        cfg->mode = suggested;
        return 0;
    }

    {
        int choice = 0;
        int parsed = parse_menu_index(input, 2, &choice);
        if (parsed == 1 && choice == 2) {
            cfg->mode = MODE_UPGRADE;
        } else if (parsed == 1 && choice == 1) {
            cfg->mode = MODE_NEW;
        } else {
            cfg->mode = suggested;
        }
    }

    return 0;
}

static int prompt_layout(struct config *cfg) {
    if (cfg->layout_set) {
        log("Esquema de particiones preseleccionado por CLI: %s", layout_label(cfg->layout));
        return 0;
    }

    if (cfg->auto_yes) {
        cfg->layout = LAYOUT_SIMPLE;
        log("--yes activo: usando esquema simple por defecto.");
        return 0;
    }

    char input[32];
    char efi_label[16];
    part1_size_label(efi_label, sizeof(efi_label));
    log("Seleccione el esquema de particiones:");
    log("  [1] Simple    -> EFI %s + ROOT resto", efi_label);
    log("  [2] Avanzado  -> EFI %s + ROOT + HOME + SWAP", efi_label);
    log("Opción recomendada [1]:");
    if (safe_flush_input(input, sizeof(input)) != 0 || input[0] == '\0') {
        cfg->layout = LAYOUT_SIMPLE;
        return 0;
    }

    {
        int choice = 0;
        int parsed = parse_menu_index(input, 2, &choice);
        cfg->layout = (parsed == 1 && choice == 2) ? LAYOUT_ADVANCED : LAYOUT_SIMPLE;
    }
    return 0;
}

static int prompt_table(struct config *cfg, int running_on_efi) {
    if (cfg->table_set) {
        log("Tabla de particiones preseleccionada por CLI: %s", table_label(cfg->table));
        return 0;
    }

    enum table_mode suggested = running_on_efi ? TABLE_GPT : TABLE_MBR;
    log("Entorno de arranque detectado: %s", running_on_efi ? "EFI/UEFI" : "BIOS/Legacy");
    log("Se recomienda %s.", table_label(suggested));

    if (cfg->auto_yes) {
        cfg->table = suggested;
        log("--yes activo: usando tabla sugerida (%s).", table_label(cfg->table));
        return 0;
    }

    char input[32];
    log("Seleccione la tabla de particiones:");
    log("  [1] MBR/BIOS");
    log("  [2] GPT/UEFI");
    log("Opción recomendada [%d]:", suggested == TABLE_MBR ? 1 : 2);
    if (safe_flush_input(input, sizeof(input)) != 0 || input[0] == '\0') {
        cfg->table = suggested;
        return 0;
    }

    {
        int choice = 0;
        int parsed = parse_menu_index(input, 2, &choice);
        if (parsed == 1 && choice == 1) {
            cfg->table = TABLE_MBR;
        } else if (parsed == 1 && choice == 2) {
            cfg->table = TABLE_GPT;
        } else {
            cfg->table = suggested;
        }
    }
    return 0;
}

static int ask_yes_no(const char *prompt, int default_yes, int auto_yes) {
    if (auto_yes) {
        log("%s %s (--yes)", prompt, default_yes ? "Sí" : "Sí");
        return 1;
    }

    char input[32];
    log("%s", prompt);
    if (safe_flush_input(input, sizeof(input)) != 0 || input[0] == '\0') {
        return default_yes;
    }
    return input[0] == 'y' || input[0] == 'Y' || input[0] == 's' || input[0] == 'S';
}

static int command_exists(const char *name) {
    const char *path = getenv("PATH");
    if (path == NULL || *path == 0) {
        return 0;
    }

    char path_copy[4096];
    strncpy(path_copy, path, sizeof(path_copy) - 1);
    path_copy[sizeof(path_copy) - 1] = 0;

    char *saveptr = NULL;
    for (char *dir = strtok_r(path_copy, ":", &saveptr);
         dir != NULL;
         dir = strtok_r(NULL, ":", &saveptr)) {
        char candidate[4096];
        snprintf(candidate, sizeof(candidate), "%s/%s", dir, name);
        if (access(candidate, X_OK) == 0) {
            return 1;
        }
    }

    return 0;
}

static int run_command_logged(const char *cmd, int dry_run) {
    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] %s" COLOR_RESET, cmd);
        return 0;
    }

    char full_cmd[4096];
    snprintf(full_cmd, sizeof(full_cmd), "%s 2>&1", cmd);
    FILE *pipe = popen(full_cmd, "r");
    if (pipe == NULL) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo ejecutar comando: %s" COLOR_RESET, cmd);
        return -1;
    }

    char line[LINE_BUF];
    while (fgets(line, sizeof(line), pipe) != NULL) {
        trim_newline(line);
        if (line[0] != 0) {
            log("%s", line);
        }
    }

    int status = pclose(pipe);
    if (status == -1) {
        log(COLOR_RED COLOR_BOLD "ERROR: pclose() falló para: %s" COLOR_RESET, cmd);
        return -1;
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: comando falló (%d): %s" COLOR_RESET,
            WIFEXITED(status) ? WEXITSTATUS(status) : status, cmd);
        return -1;
    }

    return 0;
}

static int run_command_capture_token(const char *cmd, char *out, size_t out_size) {
    FILE *pipe = popen(cmd, "r");
    if (pipe == NULL) {
        return -1;
    }

    char token[SHA256_HEX_LEN + 2];
    int scanned = fscanf(pipe, "%64s", token);
    int status = pclose(pipe);
    if (scanned != 1 || status == -1 || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        return -1;
    }

    strncpy(out, token, out_size - 1);
    out[out_size - 1] = 0;
    return 0;
}

static int verify_sha256_file(const char *image_path) {
    char sha_path[256];
    snprintf(sha_path, sizeof(sha_path), "%s.sha256", image_path);

    if (!struct_file_exists(sha_path)) {
        log(COLOR_YELLOW "ADVERTENCIA: No existe %s; se omite verificación SHA-256 previa." COLOR_RESET, sha_path);
        return 0;
    }

    FILE *fp = fopen(sha_path, "r");
    if (fp == NULL) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo abrir %s." COLOR_RESET, sha_path);
        return -1;
    }

    char expected[SHA256_HEX_LEN + 1];
    if (fscanf(fp, "%64s", expected) != 1) {
        fclose(fp);
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo leer el hash esperado de %s." COLOR_RESET, sha_path);
        return -1;
    }
    fclose(fp);

    char actual[SHA256_HEX_LEN + 1];
    if (sha256_file_path(image_path, actual) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo calcular SHA-256 de %s." COLOR_RESET, image_path);
        return -1;
    }

    if (strcasecmp(expected, actual) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: SHA-256 no coincide para %s." COLOR_RESET, image_path);
        log("  Esperado: %s", expected);
        log("  Actual:   %s", actual);
        return -1;
    }

    log(COLOR_GREEN "SHA-256 verificado correctamente para %s." COLOR_RESET, image_path);
    return 0;
}

static uint64_t last_partitionable_lba(uint64_t disk_sectors, enum table_mode table) {
    if (table == TABLE_GPT && disk_sectors > GPT_BACKUP_RESERVED_SECTORS) {
        return disk_sectors - GPT_BACKUP_RESERVED_SECTORS;
    }
    return disk_sectors - 1ULL;
}

static uint64_t sectors_from_start(uint64_t disk_sectors, uint64_t start_sector, enum table_mode table) {
    uint64_t last_lba = last_partitionable_lba(disk_sectors, table);

    if (start_sector > last_lba) {
        return 0;
    }
    return last_lba - start_sector + 1ULL;
}

static int build_partition_plan(enum layout_mode layout, enum table_mode table, uint64_t disk_sectors,
                                struct partition_plan *parts, int *part_count,
                                char *summary, size_t summary_size) {
    uint64_t last_lba = last_partitionable_lba(disk_sectors, table);

    memset(parts, 0, sizeof(struct partition_plan) * 4);

    if (disk_sectors < PART1_START + PART1_SECTORS + 4096ULL) {
        log(COLOR_RED COLOR_BOLD "ERROR: El disco es demasiado pequeño para instalar Eclipse OS." COLOR_RESET);
        return -1;
    }
    if (table == TABLE_GPT && last_lba < PART1_START + PART1_SECTORS + 2048ULL) {
        log(COLOR_RED COLOR_BOLD "ERROR: El disco es demasiado pequeño para GPT (se reservan %llu sectores al final)."
            COLOR_RESET,
            (unsigned long long)GPT_BACKUP_RESERVED_SECTORS);
        return -1;
    }

    strcpy(parts[0].name, "EFI");
    parts[0].start_sector = PART1_START;
    parts[0].sectors = PART1_SECTORS;
    parts[0].mbr_type = 0xEF;
    parts[0].gpt_type = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
    parts[0].image_path = EFI_IMAGE_GZ;
    parts[0].write_image = 1;
    parts[0].verify_after_write = 1;

    uint64_t next_start = align_up_u64(parts[0].start_sector + parts[0].sectors, 2048ULL);

    if (layout == LAYOUT_SIMPLE) {
        strcpy(parts[1].name, "ROOT");
        parts[1].start_sector = next_start;
        parts[1].sectors = sectors_from_start(disk_sectors, parts[1].start_sector, table);
        if (parts[1].sectors < 2048ULL) {
            log(COLOR_RED COLOR_BOLD "ERROR: No hay espacio suficiente para ROOT." COLOR_RESET);
            return -1;
        }
        parts[1].mbr_type = 0x83;
        parts[1].gpt_type = "0FC63DAF-8483-4772-8E79-3D69D8477DE4";
        parts[1].image_path = ROOTFS_IMAGE_GZ;
        parts[1].write_image = 1;
        parts[1].verify_after_write = 1;
        *part_count = 2;
        {
            char efi_label[16];
            part1_size_label(efi_label, sizeof(efi_label));
            snprintf(summary, summary_size, "Simple (EFI %s + ROOT resto)", efi_label);
        }
        return 0;
    }

    uint64_t swap_target = (disk_sectors * 5ULL) / 100ULL;
    uint64_t max_swap = (2ULL * GIB) / SECTOR_SIZE;
    if (swap_target > max_swap) {
        swap_target = max_swap;
    }
    swap_target = align_up_u64(swap_target, 2048ULL);

    uint64_t root_target;
    if (disk_sectors * SECTOR_SIZE >= 30ULL * GIB) {
        root_target = (10ULL * GIB) / SECTOR_SIZE;
    } else {
        uint64_t remaining = (last_lba + 1ULL) - next_start;
        root_target = remaining / 2ULL;
    }
    root_target = align_up_u64(root_target, 2048ULL);

    uint64_t swap_start = align_down_u64(last_lba + 1ULL - swap_target, 2048ULL);
    if (swap_start <= next_start + 4096ULL) {
        log(COLOR_RED COLOR_BOLD "ERROR: El disco es demasiado pequeño para el esquema avanzado." COLOR_RESET);
        return -1;
    }

    if (next_start + root_target + 2048ULL >= swap_start) {
        root_target = align_down_u64((swap_start - next_start) / 2ULL, 2048ULL);
    }
    if (root_target < 2048ULL) {
        log(COLOR_RED COLOR_BOLD "ERROR: No hay espacio suficiente para ROOT en el esquema avanzado." COLOR_RESET);
        return -1;
    }

    strcpy(parts[1].name, "ROOT");
    parts[1].start_sector = next_start;
    parts[1].sectors = root_target;
    parts[1].mbr_type = 0x83;
    parts[1].gpt_type = "0FC63DAF-8483-4772-8E79-3D69D8477DE4";
    parts[1].image_path = ROOTFS_IMAGE_GZ;
    parts[1].write_image = 1;
    parts[1].verify_after_write = 1;

    strcpy(parts[2].name, "HOME");
    parts[2].start_sector = align_up_u64(parts[1].start_sector + parts[1].sectors, 2048ULL);
    if (parts[2].start_sector >= swap_start) {
        log(COLOR_RED COLOR_BOLD "ERROR: No hay espacio suficiente para HOME en el esquema avanzado." COLOR_RESET);
        return -1;
    }
    parts[2].sectors = swap_start - parts[2].start_sector;
    parts[2].mbr_type = 0x83;
    parts[2].gpt_type = "0FC63DAF-8483-4772-8E79-3D69D8477DE4";
    parts[2].image_path = NULL;
    parts[2].write_image = 0;
    parts[2].verify_after_write = 0;

    strcpy(parts[3].name, "SWAP");
    parts[3].start_sector = swap_start;
    parts[3].sectors = last_lba - swap_start + 1ULL;
    parts[3].mbr_type = 0x82;
    parts[3].gpt_type = "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F";
    parts[3].image_path = NULL;
    parts[3].write_image = 0;
    parts[3].verify_after_write = 0;

    *part_count = 4;
    {
        char efi_label[16];
        part1_size_label(efi_label, sizeof(efi_label));
        snprintf(summary, summary_size, "Avanzado (EFI %s + ROOT + HOME + SWAP)", efi_label);
    }
    return 0;
}

#define GPT_HEADER_SIZE 92U
#define GPT_NUM_ENTRIES 128U
#define GPT_ENTRY_SIZE 128U
#define GPT_ENTRY_SECTORS 32U

typedef struct {
    uint64_t signature;
    uint32_t revision;
    uint32_t header_size;
    uint32_t header_crc32;
    uint32_t reserved;
    uint64_t my_lba;
    uint64_t alternate_lba;
    uint64_t first_usable_lba;
    uint64_t last_usable_lba;
    uint8_t disk_guid[16];
    uint64_t partition_entry_lba;
    uint32_t num_partition_entries;
    uint32_t size_of_partition_entry;
    uint32_t partition_entry_array_crc32;
} __attribute__((packed)) gpt_header;

typedef struct {
    uint8_t type_guid[16];
    uint8_t partition_guid[16];
    uint64_t first_lba;
    uint64_t last_lba;
    uint64_t attributes;
    uint16_t name[36];
} __attribute__((packed)) gpt_partition_entry;

static uint32_t gpt_crc32(const uint8_t *data, size_t len) {
    uint32_t crc = 0xffffffffU;
    size_t i;
    int bit;

    for (i = 0; i < len; i++) {
        crc ^= data[i];
        for (bit = 0; bit < 8; bit++) {
            if (crc & 1U) {
                crc = (crc >> 1) ^ 0xedb88320U;
            } else {
                crc >>= 1;
            }
        }
    }
    return ~crc;
}

static int parse_gpt_uuid(const char *text, uint8_t out[16]) {
    unsigned int data1;
    unsigned int data2;
    unsigned int data3;
    unsigned int b[8];

    if (text == NULL || out == NULL) {
        return -1;
    }
    if (sscanf(text, "%8x-%4x-%4x-%2x%2x-%2x%2x%2x%2x%2x%2x",
               &data1, &data2, &data3, &b[0], &b[1], &b[2], &b[3], &b[4], &b[5], &b[6], &b[7]) != 11) {
        return -1;
    }
    out[0] = (uint8_t)(data1 & 0xffU);
    out[1] = (uint8_t)((data1 >> 8) & 0xffU);
    out[2] = (uint8_t)((data1 >> 16) & 0xffU);
    out[3] = (uint8_t)((data1 >> 24) & 0xffU);
    out[4] = (uint8_t)(data2 & 0xffU);
    out[5] = (uint8_t)((data2 >> 8) & 0xffU);
    out[6] = (uint8_t)(data3 & 0xffU);
    out[7] = (uint8_t)((data3 >> 8) & 0xffU);
    out[8] = (uint8_t)b[0];
    out[9] = (uint8_t)b[1];
    out[10] = (uint8_t)b[2];
    out[11] = (uint8_t)b[3];
    out[12] = (uint8_t)b[4];
    out[13] = (uint8_t)b[5];
    out[14] = (uint8_t)b[6];
    out[15] = (uint8_t)b[7];
    return 0;
}

static void utf16le_name(const char *ascii, uint16_t out[36]) {
    size_t i;

    for (i = 0; i < 35 && ascii != NULL && ascii[i] != '\0'; i++) {
        out[i] = (uint16_t)ascii[i];
    }
    out[i] = 0;
}

static void generate_guid(uint8_t guid[16], uint64_t salt) {
    uint64_t t = (uint64_t)time(NULL) ^ salt;
    size_t i;

    for (i = 0; i < 16; i++) {
        t = t * 6364136223846793005ULL + 1ULL;
        guid[i] = (uint8_t)(t >> 33);
    }
    guid[6] = (uint8_t)((guid[6] & 0x0fU) | 0x40U);
    guid[8] = (uint8_t)((guid[8] & 0x3fU) | 0x80U);
}

static ssize_t disk_pwrite_all(int fd, const void *buf, size_t len, uint64_t offset) {
    const uint8_t *p = buf;
    size_t left = len;

    while (left > 0) {
        ssize_t n = pwrite(fd, p, left, (off_t)offset);
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            if (errno == EINVAL || errno == ENOSYS || errno == ESPIPE) {
                if (lseek(fd, (off_t)offset, SEEK_SET) < 0) {
                    return -1;
                }
                n = write(fd, p, left);
                if (n < 0) {
                    if (errno == EINTR) {
                        continue;
                    }
                    return -1;
                }
            } else {
                return -1;
            }
        }
        if (n == 0) {
            errno = EIO;
            return -1;
        }
        p += (size_t)n;
        offset += (uint64_t)n;
        left -= (size_t)n;
    }
    return (ssize_t)len;
}

static int write_disk_lba(int fd, uint64_t lba, const void *buf, size_t len) {
    if (disk_pwrite_all(fd, buf, len, lba * SECTOR_SIZE) != (ssize_t)len) {
        return -1;
    }
    return 0;
}

static int write_gpt_partition_table_native(const char *disk_path, const struct partition_plan *parts, int part_count) {
    uint64_t disk_bytes = get_disk_size_bytes(disk_path);
    uint64_t last_lba;
    uint64_t backup_entries_lba;
    uint64_t first_usable;
    uint64_t last_usable;
    uint8_t mbr[SECTOR_SIZE];
    gpt_header primary;
    gpt_header backup;
    gpt_partition_entry entries[GPT_NUM_ENTRIES];
    uint8_t disk_guid[16];
    int fd;
    int i;

    if (disk_bytes < (PART1_START + PART1_SECTORS + 4096ULL) * SECTOR_SIZE) {
        log(COLOR_RED COLOR_BOLD "ERROR: El disco es demasiado pequeño para GPT." COLOR_RESET);
        return -1;
    }
    if (part_count <= 0 || part_count > (int)GPT_NUM_ENTRIES) {
        log(COLOR_RED COLOR_BOLD "ERROR: Número de particiones GPT inválido." COLOR_RESET);
        return -1;
    }

    last_lba = (disk_bytes / SECTOR_SIZE) - 1ULL;
    if (last_lba < GPT_ENTRY_SECTORS + 34ULL) {
        log(COLOR_RED COLOR_BOLD "ERROR: El disco es demasiado pequeño para la cabecera GPT de respaldo." COLOR_RESET);
        return -1;
    }

    backup_entries_lba = last_lba - GPT_ENTRY_SECTORS;
    first_usable = GPT_ENTRY_SECTORS + 2ULL;
    last_usable = backup_entries_lba - 1ULL;

    memset(mbr, 0, sizeof(mbr));
    {
        struct partition_entry *pe = (struct partition_entry *)&mbr[446];
        pe->type = 0xee;
        pe->starting_lba = 1;
        pe->sectors_count = (last_lba > UINT32_MAX) ? UINT32_MAX : (uint32_t)last_lba;
    }
    mbr[510] = 0x55;
    mbr[511] = 0xaa;

    memset(entries, 0, sizeof(entries));
    generate_guid(disk_guid, 0x4750545f4449534bULL);
    for (i = 0; i < part_count; i++) {
        uint64_t end_lba = parts[i].start_sector + parts[i].sectors - 1ULL;

        if (parts[i].start_sector < first_usable || end_lba > last_usable) {
            log(COLOR_RED COLOR_BOLD "ERROR: La partición %s queda fuera del rango GPT utilizable." COLOR_RESET,
                parts[i].name);
            return -1;
        }
        if (parse_gpt_uuid(parts[i].gpt_type, entries[i].type_guid) != 0) {
            log(COLOR_RED COLOR_BOLD "ERROR: GUID GPT inválido para %s." COLOR_RESET, parts[i].name);
            return -1;
        }
        generate_guid(entries[i].partition_guid, (uint64_t)i + 1ULL);
        entries[i].first_lba = parts[i].start_sector;
        entries[i].last_lba = end_lba;
        entries[i].attributes = 0;
        {
            uint16_t part_name[36];
            utf16le_name(parts[i].name, part_name);
            memcpy(entries[i].name, part_name, sizeof(part_name));
        }
    }

    memset(&primary, 0, sizeof(primary));
    primary.signature = 0x5452415020494645ULL; /* "EFI PART" */
    primary.revision = 0x00010000U;
    primary.header_size = GPT_HEADER_SIZE;
    primary.my_lba = 1;
    primary.alternate_lba = last_lba;
    primary.first_usable_lba = first_usable;
    primary.last_usable_lba = last_usable;
    memcpy(primary.disk_guid, disk_guid, sizeof(disk_guid));
    primary.partition_entry_lba = 2;
    primary.num_partition_entries = GPT_NUM_ENTRIES;
    primary.size_of_partition_entry = GPT_ENTRY_SIZE;
    primary.partition_entry_array_crc32 = gpt_crc32((const uint8_t *)entries, sizeof(entries));
    primary.header_crc32 = gpt_crc32((const uint8_t *)&primary, GPT_HEADER_SIZE);

    backup = primary;
    backup.my_lba = last_lba;
    backup.alternate_lba = 1;
    backup.partition_entry_lba = backup_entries_lba;
    backup.header_crc32 = 0;
    backup.header_crc32 = gpt_crc32((const uint8_t *)&backup, GPT_HEADER_SIZE);

    fd = open(disk_path, O_RDWR);
    if (fd < 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo abrir %s (%s)." COLOR_RESET, disk_path, strerror(errno));
        return -1;
    }

    if (write_disk_lba(fd, 0, mbr, sizeof(mbr)) != 0 ||
        write_disk_lba(fd, 1, &primary, sizeof(primary)) != 0 ||
        write_disk_lba(fd, 2, entries, sizeof(entries)) != 0 ||
        write_disk_lba(fd, backup_entries_lba, entries, sizeof(entries)) != 0 ||
        write_disk_lba(fd, last_lba, &backup, sizeof(backup)) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: Falló la escritura de la tabla GPT en %s (%s)." COLOR_RESET,
            disk_path, strerror(errno));
        close(fd);
        return -1;
    }

    if (fsync(fd) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: fsync falló tras escribir GPT (%s)." COLOR_RESET, strerror(errno));
        close(fd);
        return -1;
    }

    close(fd);
    log(COLOR_GREEN "Tabla GPT escrita correctamente (escritor integrado)." COLOR_RESET);
    return 0;
}

static int prepare_gpt_or_fallback(enum table_mode *table) {
    (void)table;
    return 0;
}

static int write_mbr_partition_table(const char *disk_path, const struct partition_plan *parts, int part_count, int dry_run) {
    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] Se escribiría tabla MBR en %s." COLOR_RESET, disk_path);
        return 0;
    }

    uint8_t mbr[SECTOR_SIZE];
    memset(mbr, 0, sizeof(mbr));

    for (int i = 0; i < part_count && i < 4; i++) {
        if (parts[i].start_sector > UINT32_MAX || parts[i].sectors > UINT32_MAX) {
            log(COLOR_RED COLOR_BOLD "ERROR: La partición %s excede los límites de MBR." COLOR_RESET, parts[i].name);
            return -1;
        }

        struct partition_entry *pe = (struct partition_entry *)&mbr[446 + (i * 16)];
        pe->boot_indicator = (i == 0) ? 0x80 : 0x00;
        pe->starting_chs[0] = 0x00;
        pe->starting_chs[1] = 0x02;
        pe->starting_chs[2] = 0x00;
        pe->type = parts[i].mbr_type;
        pe->ending_chs[0] = 0x00;
        pe->ending_chs[1] = 0x02;
        pe->ending_chs[2] = 0x00;
        pe->starting_lba = (uint32_t)parts[i].start_sector;
        pe->sectors_count = (uint32_t)parts[i].sectors;
    }

    mbr[510] = 0x55;
    mbr[511] = 0xAA;

    int fd = open(disk_path, O_RDWR);
    if (fd < 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo abrir %s (%s)." COLOR_RESET, disk_path, strerror(errno));
        return -1;
    }

    ssize_t written = write(fd, mbr, sizeof(mbr));
    if (written != (ssize_t)sizeof(mbr)) {
        log(COLOR_RED COLOR_BOLD "ERROR: Falló la escritura del MBR en %s (%s)." COLOR_RESET,
            disk_path, strerror(errno));
        close(fd);
        return -1;
    }

    if (fsync(fd) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: fsync falló tras escribir MBR (%s)." COLOR_RESET, strerror(errno));
        close(fd);
        return -1;
    }

    close(fd);
    log(COLOR_GREEN "Tabla MBR escrita correctamente." COLOR_RESET);
    return 0;
}

static int write_gpt_partition_table(const char *disk_path, const struct partition_plan *parts, int part_count, int dry_run) {
    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] Se escribiría tabla GPT en %s." COLOR_RESET, disk_path);
        return 0;
    }

    if (write_gpt_partition_table_native(disk_path, parts, part_count) == 0) {
        return 0;
    }

    if (command_exists("sgdisk")) {
        char cmd[4096];
        int pos = snprintf(cmd, sizeof(cmd), "sgdisk -og");
        for (int i = 0; i < part_count && pos > 0 && pos < (int)sizeof(cmd); i++) {
            uint64_t end_sector = parts[i].start_sector + parts[i].sectors - 1ULL;
            pos += snprintf(cmd + pos, sizeof(cmd) - (size_t)pos,
                            " -n %d:%" PRIu64 ":%" PRIu64 " -t %d:%s -c %d:%s",
                            i + 1, parts[i].start_sector, end_sector,
                            i + 1, parts[i].gpt_type,
                            i + 1, parts[i].name);
        }
        snprintf(cmd + strlen(cmd), sizeof(cmd) - strlen(cmd), " %s", disk_path);
        log(COLOR_YELLOW "Reintentando GPT con sgdisk externo..." COLOR_RESET);
        return run_command_logged(cmd, dry_run);
    }

    if (command_exists("gdisk")) {
        char cmd[4096];
        char script[2048];
        int spos = snprintf(script, sizeof(script), "o\\ny\\n");
        for (int i = 0; i < part_count && spos > 0 && spos < (int)sizeof(script); i++) {
            uint64_t end_sector = parts[i].start_sector + parts[i].sectors - 1ULL;
            const char *type_code = (parts[i].mbr_type == 0xEF) ? "ef00" :
                                    (parts[i].mbr_type == 0x82 ? "8200" : "8300");
            spos += snprintf(script + spos, sizeof(script) - (size_t)spos,
                             "n\\n%d\\n%" PRIu64 "\\n%" PRIu64 "\\n%s\\n",
                             i + 1, parts[i].start_sector, end_sector, type_code);
        }
        snprintf(script + strlen(script), sizeof(script) - strlen(script), "w\\ny\\n");
        snprintf(cmd, sizeof(cmd), "sh -c \"printf %%b %s | gdisk %s\"", script, disk_path);
        log(COLOR_YELLOW "Reintentando GPT con gdisk externo..." COLOR_RESET);
        return run_command_logged(cmd, dry_run);
    }

    log(COLOR_RED COLOR_BOLD "ERROR: No se pudo escribir la tabla GPT." COLOR_RESET);
    return -1;
}

static int decompress_gz_to_path(const char *gz_path, const char *out_path, int dry_run) {
    uint8_t buf[4096];
    gzFile gz;
    int out_fd;
    size_t total = 0;
    off_t file_size = 0;
    int last_pct = -1;

    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] gzip -dc %s -> %s" COLOR_RESET, gz_path, out_path);
        return 0;
    }

    struct stat st;
    if (stat(gz_path, &st) == 0) {
        file_size = st.st_size;
    }

    gz = gzopen(gz_path, "rb");
    if (gz == NULL) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo abrir %s para descompresión." COLOR_RESET, gz_path);
        return -1;
    }

    out_fd = open(out_path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0644);
    if (out_fd < 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo crear %s (%s)." COLOR_RESET,
            out_path, strerror(errno));
        gzclose(gz);
        return -1;
    }

    for (;;) {
        int nread = gzread(gz, buf, sizeof(buf));
        if (nread < 0) {
            if (last_pct >= 0) {
                fprintf(stdout, "\r\x1b[K");
                fflush(stdout);
            }
            log(COLOR_RED COLOR_BOLD "ERROR: Descompresión de %s falló (%s)." COLOR_RESET,
                gz_path, gzerror(gz, NULL));
            close(out_fd);
            gzclose(gz);
            return -1;
        }
        if (nread == 0) {
            break;
        }

        ssize_t nwritten = write(out_fd, buf, (size_t)nread);
        if (nwritten < 0 || (size_t)nwritten != (size_t)nread) {
            if (last_pct >= 0) {
                fprintf(stdout, "\r\x1b[K");
                fflush(stdout);
            }
            log(COLOR_RED COLOR_BOLD "ERROR: Escritura en %s falló (%s)." COLOR_RESET,
                out_path, strerror(errno));
            close(out_fd);
            gzclose(gz);
            return -1;
        }
        total += (size_t)nread;

        if (file_size > 0) {
            z_off_t curr = gzoffset(gz);
            if (curr >= 0) {
                int pct = (int)((curr * 100) / file_size);
                if (pct > 100) {
                    pct = 100;
                }
                if (pct > last_pct) {
                    fprintf(stdout, "\rDescomprimiendo %s: %d%%...", disk_basename(gz_path), pct);
                    fflush(stdout);
                    last_pct = pct;
                }
            }
        }
    }

    if (last_pct >= 0) {
        fprintf(stdout, "\r\x1b[K");
        fflush(stdout);
    }

    if (fsync(out_fd) != 0) {
        log(COLOR_YELLOW "ADVERTENCIA: fsync en %s: %s" COLOR_RESET, out_path, strerror(errno));
    }
    close(out_fd);
    if (gzclose(gz) != Z_OK) {
        log(COLOR_RED COLOR_BOLD "ERROR: Cierre de %s falló." COLOR_RESET, gz_path);
        return -1;
    }

    log("Descomprimidos %zu bytes de %s a %s.", total, gz_path, out_path);
    return 0;
}

static int write_gz_image_to_disk(const char *gz_path, const char *disk_path, uint64_t start_sector, int dry_run) {
    uint8_t buf[4096];
    gzFile gz;
    uint64_t offset;
    size_t total = 0;
    int disk_fd;

    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] gzip -dc %s -> %s @ sector %" PRIu64 COLOR_RESET,
            gz_path, disk_path, start_sector);
        return 0;
    }

    gz = gzopen(gz_path, "rb");
    if (gz == NULL) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo abrir %s para descompresión." COLOR_RESET, gz_path);
        return -1;
    }

    disk_fd = open(disk_path, O_RDWR | O_CLOEXEC);
    if (disk_fd < 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo abrir %s para escritura (%s)." COLOR_RESET,
            disk_path, strerror(errno));
        gzclose(gz);
        return -1;
    }

    offset = start_sector * SECTOR_SIZE;
    for (;;) {
        int nread = gzread(gz, buf, sizeof(buf));
        if (nread < 0) {
            log(COLOR_RED COLOR_BOLD "ERROR: Descompresión de %s falló (%s)." COLOR_RESET,
                gz_path, gzerror(gz, NULL));
            close(disk_fd);
            gzclose(gz);
            return -1;
        }
        if (nread == 0) {
            break;
        }

        if (disk_pwrite_all(disk_fd, buf, (size_t)nread, offset) != nread) {
            log(COLOR_RED COLOR_BOLD "ERROR: Escritura en %s @ %" PRIu64 " falló (%s)." COLOR_RESET,
                disk_path, offset, strerror(errno));
            close(disk_fd);
            gzclose(gz);
            return -1;
        }

        offset += (uint64_t)nread;
        total += (size_t)nread;
    }

    if (fsync(disk_fd) != 0) {
        log(COLOR_YELLOW "ADVERTENCIA: fsync en %s: %s" COLOR_RESET, disk_path, strerror(errno));
    }
    close(disk_fd);
    if (gzclose(gz) != Z_OK) {
        log(COLOR_RED COLOR_BOLD "ERROR: Cierre de %s falló." COLOR_RESET, gz_path);
        return -1;
    }

    log("Escritos %zu bytes en %s (sector inicial %" PRIu64 ").", total, disk_path, start_sector);
    return 0;
}

static int write_partition_image_with_retry(const char *part_path, const struct partition_plan *part, int dry_run) {
    for (int attempt = 1; attempt <= 3; attempt++) {
        if (write_gz_image_to_disk(part->image_path, part_path, 0, dry_run) == 0) {
            log(COLOR_GREEN "%s escrita correctamente en intento %d." COLOR_RESET, part->name, attempt);
            return 0;
        }
        log(COLOR_YELLOW "Intento %d/3 falló al escribir %s; reintentando..." COLOR_RESET, attempt, part->name);
    }

    log(COLOR_RED COLOR_BOLD "ERROR: Se agotaron los reintentos al escribir %s." COLOR_RESET, part->name);
    return -1;
}

static int verify_partition_write(const char *part_path, const struct partition_plan *part, int dry_run) {
    uint8_t disk_buf[VERIFY_PREFIX_BYTES];
    uint8_t src_buf[VERIFY_PREFIX_BYTES];
    char disk_sha[SHA256_HEX_LEN + 1];
    char src_sha[SHA256_HEX_LEN + 1];
    uint64_t offset;
    int fd;

    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] Verificación posterior de %s (primeros %u bytes)." COLOR_RESET,
            part->name, VERIFY_PREFIX_BYTES);
        return 0;
    }

    if (part->image_path == NULL) {
        return 0;
    }

    fd = open(part_path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo abrir %s para verificación (%s)." COLOR_RESET,
            part_path, strerror(errno));
        return -1;
    }

    offset = 0;
    if (disk_pread_all(fd, disk_buf, sizeof(disk_buf), offset) != (ssize_t)sizeof(disk_buf)) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo leer %s @ %" PRIu64 " para verificación (%s)." COLOR_RESET,
            part_path, offset, strerror(errno));
        close(fd);
        return -1;
    }
    close(fd);

    if (read_gz_prefix(part->image_path, src_buf, sizeof(src_buf)) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo leer %s para verificación." COLOR_RESET, part->image_path);
        return -1;
    }

    sha256_hex_buffer(disk_buf, sizeof(disk_buf), disk_sha);
    sha256_hex_buffer(src_buf, sizeof(src_buf), src_sha);

    if (strcasecmp(disk_sha, src_sha) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: Verificación post-escritura falló para %s." COLOR_RESET, part->name);
        log("  source64k: %s", src_sha);
        log("  disk64k:   %s", disk_sha);
        return -1;
    }

    log(COLOR_GREEN "Verificación post-escritura correcta para %s." COLOR_RESET, part->name);
    return 0;
}

static int partition_dev_path(const char *disk_path, int part_num, char *out, size_t out_size) {
    const char *base = disk_basename(disk_path);

    if (part_num <= 0) {
        return -1;
    }
    if (strstr(base, "nvme") != NULL) {
        return snprintf(out, out_size, "%sp%d", disk_path, part_num) >= (int)out_size ? -1 : 0;
    }
    return snprintf(out, out_size, "%s%d", disk_path, part_num) >= (int)out_size ? -1 : 0;
}

static int gpt_entry_is_type(const gpt_partition_entry *entry, const char *type_uuid) {
    uint8_t expected[16];

    if (entry == NULL || type_uuid == NULL) {
        return 0;
    }
    if (entry->first_lba == 0 && entry->last_lba == 0) {
        return 0;
    }
    if (parse_gpt_uuid(type_uuid, expected) != 0) {
        return 0;
    }
    return memcmp(entry->type_guid, expected, 16) == 0;
}

static int discover_partitions(const char *disk_path, struct discovered_partitions *out) {
    unsigned char sector0[SECTOR_SIZE];
    unsigned char sector1[SECTOR_SIZE];
    int fd;
    ssize_t nread;

    memset(out, 0, sizeof(*out));

    fd = open(disk_path, O_RDONLY);
    if (fd < 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo abrir %s para localizar particiones (%s)." COLOR_RESET,
            disk_path, strerror(errno));
        return -1;
    }

    nread = pread(fd, sector1, sizeof(sector1), (off_t)SECTOR_SIZE);
    if (nread == (ssize_t)sizeof(sector1) && memcmp(sector1, "EFI PART", 8) == 0) {
        const gpt_header *hdr = (const gpt_header *)sector1;
        uint64_t entry_lba = hdr->partition_entry_lba;
        uint32_t num_entries = hdr->num_partition_entries;
        uint32_t entry_size = hdr->size_of_partition_entry;
        size_t table_bytes;
        uint8_t *table = NULL;
        uint64_t root_first_lba = UINT64_MAX;

        if (entry_size < sizeof(gpt_partition_entry) || num_entries == 0 || num_entries > GPT_NUM_ENTRIES) {
            close(fd);
            log(COLOR_YELLOW "Tabla GPT inválida; usando particiones 1 (EFI) y 2 (ROOT) por defecto." COLOR_RESET);
            out->efi_part_num = 1;
            out->root_part_num = 2;
            return 0;
        }

        table_bytes = (size_t)num_entries * (size_t)entry_size;
        table = calloc(1, table_bytes);
        if (table == NULL) {
            close(fd);
            log(COLOR_RED COLOR_BOLD "ERROR: Sin memoria para leer entradas GPT." COLOR_RESET);
            return -1;
        }

        nread = pread(fd, table, table_bytes, (off_t)(entry_lba * SECTOR_SIZE));
        close(fd);
        if (nread != (ssize_t)table_bytes) {
            free(table);
            log(COLOR_YELLOW "No se pudieron leer entradas GPT; usando particiones 1/2 por defecto." COLOR_RESET);
            out->efi_part_num = 1;
            out->root_part_num = 2;
            return 0;
        }

        for (uint32_t i = 0; i < num_entries; i++) {
            const gpt_partition_entry *entry =
                (const gpt_partition_entry *)(table + ((size_t)i * (size_t)entry_size));
            int part_num = (int)i + 1;

            if (gpt_entry_is_type(entry, GPT_ESP_TYPE)) {
                out->efi_part_num = part_num;
            }
            if (gpt_entry_is_type(entry, GPT_LINUX_TYPE) && entry->first_lba < root_first_lba) {
                root_first_lba = entry->first_lba;
                out->root_part_num = part_num;
            }
        }
        free(table);
    } else {
        nread = pread(fd, sector0, sizeof(sector0), 0);
        close(fd);
        if (nread != (ssize_t)sizeof(sector0)) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se pudo leer el MBR de %s." COLOR_RESET, disk_path);
            return -1;
        }

        for (int i = 0; i < 4; i++) {
            const struct partition_entry *pe =
                (const struct partition_entry *)&sector0[446 + (i * 16)];
            int part_num = i + 1;

            if (pe->type == 0xEF && pe->sectors_count > 0) {
                out->efi_part_num = part_num;
            }
            if (pe->type == 0x83 && pe->sectors_count > 0 && out->root_part_num == 0) {
                out->root_part_num = part_num;
            }
        }
    }

    if (out->efi_part_num == 0) {
        out->efi_part_num = 1;
        log(COLOR_YELLOW "No se encontró partición EFI; se asume la partición %d." COLOR_RESET, out->efi_part_num);
    }
    if (out->root_part_num == 0) {
        out->root_part_num = 2;
        log(COLOR_YELLOW "No se encontró partición ROOT; se asume la partición %d." COLOR_RESET, out->root_part_num);
    }

    return 0;
}

static int shell_mkdir_p(const char *path, int dry_run) {
    char cmd[512];
    snprintf(cmd, sizeof(cmd), "sh -c \"mkdir -p '%s'\"", path);
    return run_command_logged(cmd, dry_run);
}

static int shell_copy_file(const char *src, const char *dst, int dry_run) {
    uint8_t buf[64 * 1024];
    int in_fd, out_fd;
    ssize_t nread;

    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] copiar %s -> %s" COLOR_RESET, src, dst);
        return 0;
    }

    /* Copia nativa (no se usa `cp`): el driver vfat de Eclipse OS reporta el
     * mismo par (st_dev, st_ino) para distintos montajes, y `cp` aborta con
     * "are the same file" al comparar identidades. Leer/escribir directamente
     * evita por completo esa heurística. */
    in_fd = open(src, O_RDONLY | O_CLOEXEC);
    if (in_fd < 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo abrir origen %s (%s)." COLOR_RESET,
            src, strerror(errno));
        return -1;
    }

    out_fd = open(dst, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0644);
    if (out_fd < 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo crear destino %s (%s)." COLOR_RESET,
            dst, strerror(errno));
        close(in_fd);
        return -1;
    }

    while ((nread = read(in_fd, buf, sizeof(buf))) > 0) {
        ssize_t off = 0;
        while (off < nread) {
            ssize_t nwritten = write(out_fd, buf + off, (size_t)(nread - off));
            if (nwritten < 0) {
                if (errno == EINTR) {
                    continue;
                }
                log(COLOR_RED COLOR_BOLD "ERROR: Escritura en %s falló (%s)." COLOR_RESET,
                    dst, strerror(errno));
                close(in_fd);
                close(out_fd);
                return -1;
            }
            off += nwritten;
        }
    }

    if (nread < 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: Lectura de %s falló (%s)." COLOR_RESET,
            src, strerror(errno));
        close(in_fd);
        close(out_fd);
        return -1;
    }

    if (fsync(out_fd) != 0) {
        log(COLOR_YELLOW "ADVERTENCIA: fsync en %s: %s" COLOR_RESET, dst, strerror(errno));
    }
    close(in_fd);
    close(out_fd);
    return 0;
}

static int shell_mount(const char *source, const char *target, const char *fstype, int loop, int dry_run) {
    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] mount -t %s %s -> %s" COLOR_RESET, fstype ? fstype : "auto", source, target);
        return 0;
    }
    (void)loop; /* zCore mounts files/block-devices directly without loop devices */
    log("Montando %s en %s (tipo: %s)...", source, target, fstype ? fstype : "auto");
    if (mount(source, target, fstype, 0, NULL) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: mount(%s, %s, %s) falló: %s" COLOR_RESET,
            source, target, fstype ? fstype : "auto", strerror(errno));
        return -1;
    }
    return 0;
}

static int shell_umount(const char *target, int dry_run) {
    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] umount %s" COLOR_RESET, target);
        return 0;
    }
    log("Desmontando %s...", target);
    if (umount(target) != 0) {
        if (errno != EINVAL) {
            log(COLOR_YELLOW "ADVERTENCIA: umount(%s) falló: %s" COLOR_RESET, target, strerror(errno));
        }
    }
    return 0;
}

static void format_fstab_device_field(const char *device, char out[FSTAB_PLACEHOLDER_LEN + 1]) {
    size_t i;
    size_t dev_len = strlen(device);

    memset(out, ' ', FSTAB_PLACEHOLDER_LEN);
    out[FSTAB_PLACEHOLDER_LEN] = '\0';
    if (dev_len > FSTAB_PLACEHOLDER_LEN) {
        dev_len = FSTAB_PLACEHOLDER_LEN;
    }
    memcpy(out, device, dev_len);
}

typedef enum {
    FSTAB_PATCH_REPLACE = 0,
    FSTAB_PATCH_COMMENT_LINE = 1,
} fstab_patch_kind;

struct fstab_patch_target {
    const char *placeholder;
    const char *match_text;
    const char *replacement;
    fstab_patch_kind kind;
    size_t match_offset;
    int required;
    int found;
    off_t offset;
};

static int text_in_window(const uint8_t *window, size_t window_len, off_t window_base,
                          const char *text, off_t *out_offset) {
    size_t plen = strlen(text);
    size_t i;

    if (plen == 0 || window_len < plen) {
        return 0;
    }
    for (i = 0; i + plen <= window_len; i++) {
        if (memcmp(window + i, text, plen) == 0) {
            *out_offset = window_base + (off_t)i;
            return 1;
        }
    }
    return 0;
}

static int fstab_targets_ready(const struct fstab_patch_target *targets, size_t count) {
    size_t i;

    for (i = 0; i < count; i++) {
        if (targets[i].required && !targets[i].found) {
            return 0;
        }
    }
    return 1;
}

static int scan_fstab_targets(int fd, const char *block_dev, struct fstab_patch_target *targets,
                              size_t target_count) {
    uint8_t window[FSTAB_PATCH_CHUNK + FSTAB_PATCH_OVERLAP];
    size_t carry_len = 0;
    off_t file_pos = 0;
    ssize_t nread;

    log("Buscando placeholders de fstab en %s...", block_dev);
    while (file_pos < (off_t)FSTAB_PATCH_MAX_SCAN) {
        size_t room = sizeof(window) - carry_len;
        size_t chunk = room < FSTAB_PATCH_CHUNK ? room : FSTAB_PATCH_CHUNK;

        /* Use pread() with explicit offset instead of read() so that the scan
         * works correctly regardless of the fd position left by previous
         * operations (e.g. verify_partition_write) and avoids any lseek
         * side-effects on Eclipse OS block devices. */
        nread = pread(fd, window + carry_len, chunk, file_pos);
        if (nread < 0) {
            log(COLOR_RED COLOR_BOLD "ERROR: Lectura de %s falló (%s)." COLOR_RESET,
                block_dev, strerror(errno));
            return -1;
        }
        if (nread == 0 && carry_len == 0) {
            break;
        }

        {
            size_t window_len = carry_len + (nread > 0 ? (size_t)nread : 0);
            off_t window_base = file_pos - (off_t)carry_len;
            size_t t;

            for (t = 0; t < target_count; t++) {
                struct fstab_patch_target *slot = &targets[t];
                off_t hit;

                if (slot->found) {
                    continue;
                }
                if (!text_in_window(window, window_len, window_base,
                                    slot->match_text ? slot->match_text : slot->placeholder, &hit)) {
                    continue;
                }
                slot->offset = hit + (off_t)slot->match_offset;
                slot->found = 1;
            }
        }

        if (fstab_targets_ready(targets, target_count)) {
            return 0;
        }
        if (nread == 0) {
            break;
        }

        file_pos += nread;
        {
            size_t window_len = carry_len + (size_t)nread;
            if (window_len > FSTAB_PATCH_OVERLAP) {
                memmove(window, window + window_len - FSTAB_PATCH_OVERLAP, FSTAB_PATCH_OVERLAP);
                carry_len = FSTAB_PATCH_OVERLAP;
            } else {
                carry_len = window_len;
            }
        }
    }

    if (!fstab_targets_ready(targets, target_count)) {
        log(COLOR_RED COLOR_BOLD
            "ERROR: No se encontraron placeholders de fstab en %s (escaneados %lld bytes)." COLOR_RESET,
            block_dev, (long long)file_pos);
        log(COLOR_YELLOW
            "Reconstruya el medio de instalación (cargo rootfs && cargo image) para incluir /etc/fstab con placeholders." COLOR_RESET);
        return -1;
    }
    return 0;
}

static int apply_fstab_targets(int fd, const char *block_dev, uint64_t partition_offset,
                               const struct fstab_patch_target *targets, size_t target_count) {
    size_t i;

    for (i = 0; i < target_count; i++) {
        const struct fstab_patch_target *slot = &targets[i];
        uint64_t at = partition_offset + (uint64_t)slot->offset;

        if (!slot->found) {
            continue;
        }
        /* Use pwrite() with explicit offset instead of lseek()+write() to
         * avoid any seek-position issues on Eclipse OS block devices. */
        if (slot->kind == FSTAB_PATCH_COMMENT_LINE) {
            if (pwrite(fd, "#", 1, (off_t)at) != 1) {
                log(COLOR_RED COLOR_BOLD "ERROR: Escritura de fstab en %s falló (%s)." COLOR_RESET,
                    block_dev, strerror(errno));
                return -1;
            }
        } else if (pwrite(fd, slot->replacement, FSTAB_PLACEHOLDER_LEN, (off_t)at) !=
                   (ssize_t)FSTAB_PLACEHOLDER_LEN) {
            log(COLOR_RED COLOR_BOLD "ERROR: Escritura de fstab en %s falló (%s)." COLOR_RESET,
                block_dev, strerror(errno));
            return -1;
        }
    }
    if (fsync(fd) != 0) {
        log(COLOR_YELLOW "ADVERTENCIA: fsync en %s: %s" COLOR_RESET, block_dev, strerror(errno));
    }
    return 0;
}

/* Re-lee los bytes parcheados para confirmar que la sustitución llegó al
 * almacenamiento. Si un placeholder requerido sigue presente (escritura corta,
 * offset erróneo o caché incoherente), devolvemos -1 para que el llamante caiga
 * al método basado en mount en lugar de dejar el fstab con placeholders. */
static int verify_fstab_targets(int fd, const char *block_dev, uint64_t partition_offset,
                                const struct fstab_patch_target *targets, size_t target_count) {
    size_t i;

    for (i = 0; i < target_count; i++) {
        const struct fstab_patch_target *slot = &targets[i];
        uint8_t buf[FSTAB_PLACEHOLDER_LEN];
        size_t want;
        uint64_t at;

        if (!slot->found) {
            continue;
        }
        want = (slot->kind == FSTAB_PATCH_COMMENT_LINE) ? 1U : (size_t)FSTAB_PLACEHOLDER_LEN;
        at = partition_offset + (uint64_t)slot->offset;
        if (disk_pread_all(fd, buf, want, at) != (ssize_t)want) {
            log(COLOR_RED COLOR_BOLD "ERROR: lectura de verificación de fstab en %s falló (%s)." COLOR_RESET,
                block_dev, strerror(errno));
            return -1;
        }
        if (slot->kind == FSTAB_PATCH_COMMENT_LINE) {
            if (buf[0] != '#') {
                log(COLOR_RED COLOR_BOLD
                    "ERROR: verificación de fstab: la línea de %s no quedó comentada en %s." COLOR_RESET,
                    slot->placeholder, block_dev);
                return -1;
            }
        } else if (memcmp(buf, slot->replacement, FSTAB_PLACEHOLDER_LEN) != 0) {
            log(COLOR_RED COLOR_BOLD
                "ERROR: verificación de fstab: %s no se sustituyó en %s." COLOR_RESET,
                slot->placeholder, block_dev);
            return -1;
        }
    }
    return 0;
}

static int write_fstab_file(const char *path, const char *efi_dev, const char *root_dev,
                            const char *home_dev, const char *swap_dev,
                            enum layout_mode layout, int dry_run) {
    FILE *f;

    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] Escribiría fstab en %s." COLOR_RESET, path);
        return 0;
    }

    f = fopen(path, "w");
    if (f == NULL) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo abrir %s (%s)." COLOR_RESET,
            path, strerror(errno));
        return -1;
    }

    fprintf(f, "# /etc/fstab - generado por install-eclipse\n");
    fprintf(f, "# <dispositivo>  <punto de montaje>  <tipo>  <opciones>       <dump>  <pass>\n");
    fprintf(f, "%-20s  /                  btrfs   defaults          0  1\n", root_dev);
    fprintf(f, "%-20s  /boot/efi          vfat    defaults,noatime  0  0\n", efi_dev);
    if (layout == LAYOUT_ADVANCED && home_dev[0] != '\0') {
        fprintf(f, "%-20s  /home              btrfs   defaults          0  0\n", home_dev);
    }
    if (layout == LAYOUT_ADVANCED && swap_dev[0] != '\0') {
        fprintf(f, "%-20s  none               swap    sw                0  0\n", swap_dev);
    }

    if (fclose(f) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo cerrar %s (%s)." COLOR_RESET,
            path, strerror(errno));
        return -1;
    }
    return 0;
}

static int write_fstab_via_mount(const char *root_dev, const char *efi_dev,
                                 const char *home_dev, const char *swap_dev,
                                 enum layout_mode layout, int dry_run) {
    char path[512];

    if (shell_mkdir_p(INST_ROOT_MNT, dry_run) != 0) {
        return -1;
    }
    /* ROOT instalado: btrfs (único FS de disco soportado). */
    if (shell_mount(root_dev, INST_ROOT_MNT, "btrfs", 0, dry_run) != 0) {
        return -1;
    }

    snprintf(path, sizeof(path), "%s/etc/fstab", INST_ROOT_MNT);
    if (write_fstab_file(path, efi_dev, root_dev, home_dev, swap_dev, layout, dry_run) != 0) {
        shell_umount(INST_ROOT_MNT, dry_run);
        return -1;
    }

    shell_umount(INST_ROOT_MNT, dry_run);
    log(COLOR_GREEN "fstab actualizado en %s (vía mount)." COLOR_RESET, root_dev);
    return 0;
}

static int patch_fstab_on_block_device(const char *block_dev,
                                       const char *efi_dev, const char *root_dev, const char *home_dev,
                                       const char *swap_dev, enum layout_mode layout, int dry_run) {
    char efi_field[FSTAB_PLACEHOLDER_LEN + 1];
    char root_field[FSTAB_PLACEHOLDER_LEN + 1];
    char home_field[FSTAB_PLACEHOLDER_LEN + 1];
    char swap_field[FSTAB_PLACEHOLDER_LEN + 1];
    struct fstab_patch_target targets[4];
    size_t target_count = 4;
    int fd = -1;
    int rc = -1;
    uint64_t partition_offset = 0;

    format_fstab_device_field(efi_dev, efi_field);
    format_fstab_device_field(root_dev, root_field);
    format_fstab_device_field(home_dev, home_field);
    format_fstab_device_field(swap_dev, swap_field);

    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] Parchearía fstab en %s (ROOT=%s, EFI=%s)." COLOR_RESET,
            block_dev, root_dev, efi_dev);
        return 0;
    }

    targets[0] = (struct fstab_patch_target){
        FSTAB_PLACEHOLDER_ROOT, FSTAB_TEMPLATE_LINE_ROOT, root_field, FSTAB_PATCH_REPLACE, 0, 1, 0, 0,
    };
    targets[1] = (struct fstab_patch_target){
        FSTAB_PLACEHOLDER_EFI, FSTAB_TEMPLATE_LINE_EFI, efi_field, FSTAB_PATCH_REPLACE, 0, 1, 0, 0,
    };
    if (layout == LAYOUT_ADVANCED) {
        targets[2] = (struct fstab_patch_target){
            FSTAB_PLACEHOLDER_HOME, FSTAB_TEMPLATE_LINE_HOME, home_field, FSTAB_PATCH_REPLACE, 0, 1, 0, 0,
        };
        targets[3] = (struct fstab_patch_target){
            FSTAB_PLACEHOLDER_SWAP, FSTAB_TEMPLATE_LINE_SWAP, swap_field, FSTAB_PATCH_REPLACE, 0, 1, 0, 0,
        };
    } else {
        targets[2] = (struct fstab_patch_target){
            FSTAB_PLACEHOLDER_HOME, FSTAB_TEMPLATE_LINE_HOME, NULL, FSTAB_PATCH_COMMENT_LINE, 0, 0, 0, 0,
        };
        targets[3] = (struct fstab_patch_target){
            FSTAB_PLACEHOLDER_SWAP, FSTAB_TEMPLATE_LINE_SWAP, NULL, FSTAB_PATCH_COMMENT_LINE, 0, 0, 0, 0,
        };
    }

    fd = open(block_dev, O_RDWR | O_CLOEXEC);
    if (fd < 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo abrir %s (%s)." COLOR_RESET,
            block_dev, strerror(errno));
        return -1;
    }

    if (scan_fstab_targets(fd, block_dev, targets, target_count) != 0) {
        goto cleanup;
    }
    if (apply_fstab_targets(fd, block_dev, partition_offset, targets, target_count) != 0) {
        goto cleanup;
    }

    /* Fuerza a disco lo recién escrito y descarta la caché de bloques. Es clave
     * por dos motivos: (1) en este entorno un fsync poco fiable podía dejar el
     * parche (una sola página) en caché sucia y perderse al reiniciar mientras la
     * imagen grande sí se vaciaba por presión de caché — exactamente el síntoma de
     * placeholders sin sustituir tras instalar; (2) obliga a que la verificación
     * siguiente lea del almacenamiento real y no de la caché. */
    sync();
    /* En Eclipse OS las escrituras de bloque van directas al dispositivo (sin
     * caché), así que BLKFLSBUF puede no estar implementado: eso es benigno. Sólo
     * avisamos ante errores inesperados. */
    if (ioctl(fd, BLKFLSBUF, 0) != 0 &&
        errno != ENOSYS && errno != ENOTTY && errno != EOPNOTSUPP) {
        log(COLOR_YELLOW "ADVERTENCIA: BLKFLSBUF en %s: %s" COLOR_RESET, block_dev, strerror(errno));
    }

    if (verify_fstab_targets(fd, block_dev, partition_offset, targets, target_count) != 0) {
        goto cleanup;
    }

    log(COLOR_GREEN "fstab actualizado y verificado (en disco) en %s (sin montar)." COLOR_RESET, block_dev);
    rc = 0;

cleanup:
    close(fd);
    return rc;
}

static int copy_upgrade_payload_if_exists(const char *src, const char *dst, int dry_run) {
    if (!struct_file_exists(src)) {
        log(COLOR_YELLOW "Omitido (no presente en el medio de instalación): %s" COLOR_RESET, src);
        return 0;
    }
    if (shell_copy_file(src, dst, dry_run) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo copiar %s -> %s." COLOR_RESET, src, dst);
        return -1;
    }
    log(COLOR_GREEN "Actualizado: %s" COLOR_RESET, dst);
    return 0;
}

static int write_fstab_to_root(const char *disk_path, const struct partition_plan *parts,
                               int part_count, enum layout_mode layout, int dry_run) {
    char efi_dev[160], root_dev[160], home_dev[160], swap_dev[160];
    char cmd[512];

    /* EFI = part 1, ROOT = part 2, HOME = part 3, SWAP = part 4 */
    if (partition_dev_path(disk_path, 1, efi_dev,  sizeof(efi_dev))  != 0 ||
        partition_dev_path(disk_path, 2, root_dev, sizeof(root_dev)) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo construir la ruta de las particiones." COLOR_RESET);
        return -1;
    }

    if (layout == LAYOUT_ADVANCED && part_count >= 4) {
        if (partition_dev_path(disk_path, 3, home_dev, sizeof(home_dev)) != 0 ||
            partition_dev_path(disk_path, 4, swap_dev, sizeof(swap_dev)) != 0) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se pudo construir la ruta de HOME/SWAP." COLOR_RESET);
            return -1;
        }

        log(COLOR_GREEN "[4/5] Formateando partición HOME (btrfs)..." COLOR_RESET);
        /* Se vuelca una plantilla btrfs vacía generada en build; el kernel la
         * expande a toda la partición en el primer montaje. */
        if (!struct_file_exists(HOME_IMAGE_GZ)) {
            log(COLOR_YELLOW
                "ADVERTENCIA: falta %s; HOME quedará sin formatear." COLOR_RESET, HOME_IMAGE_GZ);
        } else if (write_gz_image_to_disk(HOME_IMAGE_GZ, home_dev, 0, dry_run) != 0) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se pudo formatear HOME." COLOR_RESET);
            return -1;
        }

        log(COLOR_GREEN "[5/5] Inicializando partición SWAP..." COLOR_RESET);
        snprintf(cmd, sizeof(cmd), "sh -c \"mkswap -L SWAP '%s'\"", swap_dev);
        if (run_command_logged(cmd, dry_run) != 0) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se pudo inicializar SWAP." COLOR_RESET);
            return -1;
        }
    }

    log(COLOR_GREEN "Actualizando /etc/fstab en la partición ROOT..." COLOR_RESET);
    /* Parche en bruto primero: no depende de mount(2) y evita bloqueos en Eclipse OS. */
    if (layout == LAYOUT_ADVANCED && part_count >= 4) {
        if (patch_fstab_on_block_device(root_dev, efi_dev, root_dev, home_dev, swap_dev,
                                        layout, dry_run) == 0) {
            return 0;
        }
        log(COLOR_YELLOW
            "ADVERTENCIA: parche en bruto falló; intentando escribir fstab vía mount." COLOR_RESET);
        return write_fstab_via_mount(root_dev, efi_dev, home_dev, swap_dev, layout, dry_run);
    }
    if (patch_fstab_on_block_device(root_dev, efi_dev, root_dev, "", "", LAYOUT_SIMPLE, dry_run) == 0) {
        return 0;
    }
    log(COLOR_YELLOW
        "ADVERTENCIA: parche en bruto falló; intentando escribir fstab vía mount." COLOR_RESET);
    return write_fstab_via_mount(root_dev, efi_dev, "", "", LAYOUT_SIMPLE, dry_run);
}

/* Con btrfs no hay que expandir ROOT en el instalador: el kernel de Eclipse
 * detecta en el primer montaje que la partición es mayor que el sistema de
 * archivos y lo expande automáticamente (grow_to_device). Sólo aseguramos que
 * lo escrito esté en disco. */
static int resize_root_to_partition(const char *root_dev, int dry_run) {
    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] ROOT (btrfs) se expandirá en el primer montaje." COLOR_RESET);
        return 0;
    }
    sync();
    log(COLOR_GREEN
        "ROOT (btrfs, %s) se expandirá a toda la partición en el primer arranque." COLOR_RESET,
        root_dev);
    return 0;
}

/* Fija ROOT=<root_dev> en la cmdline de rboot.conf de la partición EFI,
 * sustituyendo en bruto el placeholder RBOOT_PLACEHOLDER_ROOT. Esto hace que el
 * kernel pivote de forma determinista a la partición ROOT instalada. Es una
 * operación no fatal: si el placeholder no está (imagen EFI antigua) o no se
 * puede escribir, el kernel recurre a la auto-detección del root. */
static int patch_rboot_root_on_efi(const char *block_dev, const char *root_dev, int dry_run) {
    char root_field[FSTAB_PLACEHOLDER_LEN + 1];
    struct fstab_patch_target targets[1];
    int fd = -1;
    uint64_t partition_offset = 0;

    format_fstab_device_field(root_dev, root_field);

    if (dry_run) {
        log(COLOR_YELLOW "[dry-run] Fijaría ROOT=%s en rboot.conf de %s." COLOR_RESET,
            root_dev, block_dev);
        return 0;
    }

    targets[0] = (struct fstab_patch_target){
        RBOOT_PLACEHOLDER_ROOT, RBOOT_TEMPLATE_ROOT_KEY, root_field, FSTAB_PATCH_REPLACE, 5, 1, 0, 0,
    };

    fd = open(block_dev, O_RDWR | O_CLOEXEC);
    if (fd < 0) {
        log(COLOR_YELLOW
            "ADVERTENCIA: no se pudo abrir %s para fijar ROOT= en rboot.conf (%s); se usará auto-detección." COLOR_RESET,
            block_dev, strerror(errno));
        return 0;
    }

    if (scan_fstab_targets(fd, block_dev, targets, 1) != 0) {
        log(COLOR_YELLOW
            "ADVERTENCIA: rboot.conf sin placeholder ROOT en %s; se usará auto-detección del root." COLOR_RESET,
            block_dev);
        close(fd);
        return 0;
    }
    if (apply_fstab_targets(fd, block_dev, partition_offset, targets, 1) != 0) {
        log(COLOR_YELLOW "ADVERTENCIA: no se pudo escribir ROOT= en rboot.conf de %s." COLOR_RESET,
            block_dev);
        close(fd);
        return 0;
    }

    log(COLOR_GREEN "rboot.conf: ROOT=%s fijado en %s (cmdline del kernel)." COLOR_RESET,
        root_dev, block_dev);
    close(fd);
    return 0;
}

/* Busca `mountpoint` en /proc/mounts. Devuelve 0 si está montado (copiando el
 * dispositivo de origen en dev_out, si no es NULL) o -1 si no lo está / no se
 * pudo leer /proc/mounts. El formato de cada línea es:
 *   <source> <target> <fstype> <options> 0 0 */
static int find_mount_device(const char *mountpoint, char *dev_out, size_t dev_size) {
    FILE *fp = fopen("/proc/mounts", "r");
    if (fp == NULL) {
        return -1;
    }

    char line[1024];
    int found = -1;
    while (fgets(line, sizeof(line), fp) != NULL) {
        char src[256];
        char tgt[256];
        if (sscanf(line, "%255s %255s", src, tgt) == 2 && strcmp(tgt, mountpoint) == 0) {
            if (dev_out != NULL && dev_size > 0) {
                strncpy(dev_out, src, dev_size - 1);
                dev_out[dev_size - 1] = '\0';
            }
            found = 0;
            break;
        }
    }
    fclose(fp);
    return found;
}

static int run_upgrade(const char *disk_path, int dry_run) {
    struct discovered_partitions parts;
    char efi_dev[160];
    char root_dev[160];
    int rc = 0;
    const char *src_mnt = NULL;
    int mount_needed = 0;
    int root_mounted = 0;
    char root_img_path[512] = "";

    if (discover_partitions(disk_path, &parts) != 0) {
        return -1;
    }
    if (partition_dev_path(disk_path, parts.efi_part_num, efi_dev, sizeof(efi_dev)) != 0 ||
        partition_dev_path(disk_path, parts.root_part_num, root_dev, sizeof(root_dev)) != 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: Ruta de partición demasiado larga." COLOR_RESET);
        return -1;
    }

    log("Partición EFI detectada:  %s", efi_dev);
    log("Partición ROOT detectada: %s", root_dev);

    if (struct_file_exists("/boot/efi-staging/EFI/Boot/BootX64.efi")) {
        log("Se detectaron archivos de actualización pre-extraídos en /boot/efi-staging.");
        src_mnt = "/boot/efi-staging";
    } else {
        if (!struct_file_exists(EFI_IMAGE_GZ)) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se encontró %s (fuente del bootloader/kernel)." COLOR_RESET, EFI_IMAGE_GZ);
            return -1;
        }

        /* Mount ROOT partition onto UPD_ROOT_MNT so we can write the decompressed image on disk instead of RAM (tmpfs) */
        if (shell_mkdir_p(UPD_ROOT_MNT, dry_run) != 0) {
            return -1;
        }
        if (shell_mount(root_dev, UPD_ROOT_MNT, "btrfs", 0, dry_run) != 0) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se pudo montar la partición ROOT (%s) para la descompresión." COLOR_RESET, root_dev);
            return -1;
        }
        root_mounted = 1;

        snprintf(root_img_path, sizeof(root_img_path), "%s/eclipse-upd-efi.img", UPD_ROOT_MNT);

        if (decompress_gz_to_path(EFI_IMAGE_GZ, root_img_path, dry_run) != 0) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se pudo extraer %s a la partición ROOT." COLOR_RESET, EFI_IMAGE_GZ);
            rc = -1;
            goto cleanup;
        }

        if (shell_mkdir_p(UPD_STAGING_MNT, dry_run) != 0) {
            rc = -1;
            goto cleanup;
        }

        if (shell_mount(root_img_path, UPD_STAGING_MNT, "vfat", 1, dry_run) != 0) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se pudo montar la imagen EFI extraída en disco." COLOR_RESET);
            rc = -1;
            goto cleanup;
        }
        mount_needed = 1;
        src_mnt = UPD_STAGING_MNT;
    }

    /* Destino EFI: preferir la ESP que el sistema YA tiene montada en /boot/efi
     * (p. ej. /dev/sda1). Así evitamos un segundo montaje del MISMO dispositivo
     * de bloque, que crearía dos instancias FAT con cachés independientes sobre
     * la misma partición. Si /boot/efi no está montada, la montamos nosotros. */
    char efi_dest[256];
    char mounted_dev[256] = "";
    int efi_we_mounted = 0;
    if (find_mount_device("/boot/efi", mounted_dev, sizeof(mounted_dev)) == 0) {
        snprintf(efi_dest, sizeof(efi_dest), "/boot/efi");
        log(COLOR_GREEN "ESP ya montada en /boot/efi (origen %s); se escribe directamente sin re-montar." COLOR_RESET,
            mounted_dev[0] ? mounted_dev : efi_dev);
    } else {
        if (shell_mkdir_p(UPD_EFI_MNT, dry_run) != 0) {
            rc = -1;
            goto cleanup;
        }
        if (shell_mount(efi_dev, UPD_EFI_MNT, "vfat", 0, dry_run) != 0) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se pudo montar %s." COLOR_RESET, efi_dev);
            rc = -1;
            goto cleanup;
        }
        snprintf(efi_dest, sizeof(efi_dest), "%s", UPD_EFI_MNT);
        efi_we_mounted = 1;
    }

    log(COLOR_GREEN "Actualizando bootloader, kernel e initramfs en EFI (copia selectiva, sin dd)..." COLOR_RESET);

    char dst_boot_dir[320], dst_zcore_dir[320];
    char dst_boot[400], dst_zcore[400], dst_initramfs[400];
    snprintf(dst_boot_dir, sizeof(dst_boot_dir), "%s/EFI/Boot", efi_dest);
    snprintf(dst_zcore_dir, sizeof(dst_zcore_dir), "%s/EFI/zCore", efi_dest);
    if (shell_mkdir_p(dst_boot_dir, dry_run) != 0 ||
        shell_mkdir_p(dst_zcore_dir, dry_run) != 0) {
        rc = -1;
        goto cleanup_mount;
    }

    /* Copiamos solamente el kernel y el bootloader, preservando rboot.conf */
    char src_boot[256], src_zcore[256], src_initramfs[256];
    snprintf(src_boot, sizeof(src_boot), "%s/EFI/Boot/BootX64.efi", src_mnt);
    snprintf(src_zcore, sizeof(src_zcore), "%s/EFI/zCore/zcore.elf", src_mnt);
    snprintf(src_initramfs, sizeof(src_initramfs), "%s/EFI/zCore/initramfs.img", src_mnt);
    snprintf(dst_boot, sizeof(dst_boot), "%s/EFI/Boot/BootX64.efi", efi_dest);
    snprintf(dst_zcore, sizeof(dst_zcore), "%s/EFI/zCore/zcore.elf", efi_dest);
    snprintf(dst_initramfs, sizeof(dst_initramfs), "%s/EFI/zCore/initramfs.img", efi_dest);

    if (copy_upgrade_payload_if_exists(src_boot, dst_boot, dry_run) != 0 ||
        copy_upgrade_payload_if_exists(src_zcore, dst_zcore, dry_run) != 0) {
        rc = -1;
        goto cleanup_mount;
    }
    copy_upgrade_payload_if_exists(src_initramfs, dst_initramfs, dry_run);
    if (!dry_run) {
        sync();
    }

cleanup_mount:
    /* Solo desmontamos lo que montamos nosotros; la ESP del sistema en /boot/efi
     * se deja intacta. */
    if (efi_we_mounted) {
        shell_umount(UPD_EFI_MNT, dry_run);
    }

cleanup:
    if (mount_needed) {
        shell_umount(UPD_STAGING_MNT, dry_run);
    }
    if (root_mounted) {
        if (root_img_path[0] != '\0' && !dry_run) {
            unlink(root_img_path);
        }
        shell_umount(UPD_ROOT_MNT, dry_run);
    }
    if (rc == 0) {
        /* La partición EFI ya está desmontada: fija ROOT= en rboot.conf (no fatal). */
        patch_rboot_root_on_efi(efi_dev, root_dev, dry_run);
    }
    return rc;
}

static void trigger_partition_rescan(const char *disk_path) {
    int fd = open(disk_path, O_RDONLY);
    if (fd >= 0) {
#ifndef BLKRRPART
#define BLKRRPART 0x125f
#endif
        if (ioctl(fd, BLKRRPART, 0) == 0) {
            log(COLOR_GREEN "Solicitada recarga de tabla de particiones para %s." COLOR_RESET, disk_path);
        } else {
            log(COLOR_YELLOW "ADVERTENCIA: No se pudo solicitar recarga de tabla de particiones para %s (%s)." COLOR_RESET,
                disk_path, strerror(errno));
        }
        close(fd);
    }
}

int main(int argc, char **argv) {
    struct timespec start_time;
    clock_gettime(CLOCK_MONOTONIC, &start_time);

    g_log_file = fopen("/tmp/eclipse-install.log", "a");
    if (g_log_file != NULL) {
        setvbuf(g_log_file, NULL, _IOLBF, 0);
    }
    atexit(close_log_file);

    struct config cfg;
    if (parse_args(argc, argv, &cfg) != 0) {
        return 1;
    }

    print_header();
    log(COLOR_WHITE "Buscando dispositivos de almacenamiento disponibles...\n" COLOR_RESET);

    struct disk_info disks[MAX_DISKS];
    memset(disks, 0, sizeof(disks));
    char disk_list[MAX_DISKS][128];
    memset(disk_list, 0, sizeof(disk_list));
    int found_disks = scan_disks(disks, disk_list);

    if (found_disks == 0 && !cfg.disk_set) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se detectaron discos de almacenamiento." COLOR_RESET);
        log("Asegúrese de haber iniciado la máquina con un disco adjunto.");
        return 1;
    }

    if (prompt_disk_selection(&cfg, disks, disk_list, found_disks) != 0) {
        return 1;
    }

    uint64_t disk_size_bytes = get_disk_size_bytes(cfg.disk_path);
    if (disk_size_bytes == 0) {
        log(COLOR_RED COLOR_BOLD "ERROR: No se pudo acceder al disco %s." COLOR_RESET, cfg.disk_path);
        return 1;
    }
    uint64_t disk_sectors = disk_size_bytes / SECTOR_SIZE;

    print_header();
    char disk_size_str[64];
    format_size(disk_size_str, sizeof(disk_size_str), disk_size_bytes);
    log("Disco seleccionado: " COLOR_YELLOW "%s" COLOR_RESET " (%s)", cfg.disk_path, disk_size_str);

    struct existing_install_info existing;
    if (detect_existing_install(cfg.disk_path, &existing) != 0) {
        return 1;
    }

    if (existing.has_valid_mbr && existing.has_efi_partition) {
        log(COLOR_YELLOW "Se detectó MBR válido con partición EFI en el disco." COLOR_RESET);
    }
    if (existing.has_gpt_header) {
        log(COLOR_YELLOW "Se detectó cabecera GPT existente en el disco." COLOR_RESET);
    }

    if (prompt_install_mode(&cfg, &existing) != 0) {
        return 1;
    }

    if (cfg.mode == MODE_NEW) {
        log(COLOR_RED COLOR_BOLD "¡ADVERTENCIA! La nueva instalación reparticionará el disco y borrará datos existentes." COLOR_RESET);
        if (prompt_layout(&cfg) != 0) {
            return 1;
        }
        if (prompt_table(&cfg, struct_file_exists("/sys/firmware/efi")) != 0) {
            return 1;
        }
        if (prepare_gpt_or_fallback(&cfg.table) != 0) {
            return 1;
        }
    } else {
        log(COLOR_GREEN "Modo actualización: copia selectiva de archivos (sin dd ni reparticionado)." COLOR_RESET);
    }

    int removable = is_removable_disk(cfg.disk_path);
    if (removable) {
        log(COLOR_YELLOW COLOR_BOLD "ADVERTENCIA: %s es un dispositivo extraíble (/sys/block/.../removable=1)." COLOR_RESET,
            cfg.disk_path);
        if (!cfg.auto_yes) {
            if (!ask_yes_no("¿Desea continuar con este disco extraíble? (y/N):", 0, 0)) {
                log("Instalación cancelada por el usuario.");
                return 2;
            }
        } else {
            log(COLOR_YELLOW "--yes activo: se continúa pese a la advertencia de disco extraíble." COLOR_RESET);
        }
    }

    if (cfg.mode == MODE_NEW) {
        if (!struct_file_exists(EFI_IMAGE_GZ)) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se encontró %s" COLOR_RESET, EFI_IMAGE_GZ);
            return 1;
        }
        if (!struct_file_exists(ROOTFS_IMAGE_GZ)) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se encontró %s" COLOR_RESET, ROOTFS_IMAGE_GZ);
            return 1;
        }
        if (verify_sha256_file(EFI_IMAGE_GZ) != 0) {
            return 1;
        }
        if (verify_sha256_file(ROOTFS_IMAGE_GZ) != 0) {
            return 1;
        }
    } else {
        if (!struct_file_exists(EFI_IMAGE_GZ)) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se encontró %s" COLOR_RESET, EFI_IMAGE_GZ);
            return 1;
        }
        if (verify_sha256_file(EFI_IMAGE_GZ) != 0) {
            return 1;
        }
    }

    struct partition_plan parts[4];
    int part_count = 0;
    char partition_summary[128];
    if (cfg.mode == MODE_NEW) {
        if (build_partition_plan(cfg.layout, cfg.table, disk_sectors, parts, &part_count,
                                 partition_summary, sizeof(partition_summary)) != 0) {
            return 1;
        }
    } else {
        snprintf(partition_summary, sizeof(partition_summary),
                 "Actualización selectiva (EFI: bootloader + kernel + initramfs)");
    }

    log("========================================");
    log(" RESUMEN DE INSTALACIÓN");
    log("========================================");
    log(" Disco:        %s  (%s)", cfg.disk_path, disk_size_str);
    log(" Modo:         %s", mode_label(cfg.mode));
    log(" Tabla:        %s", cfg.mode == MODE_NEW ? table_label(cfg.table) : "Sin cambios");
    log(" Particiones:  %s", partition_summary);
    log(" Dry-run:      %s", cfg.dry_run ? "Sí" : "No");
    log("========================================");

    if (cfg.auto_yes) {
        log("¿Confirmar y ejecutar? (y/N): y (--yes)");
    } else if (!ask_yes_no("¿Confirmar y ejecutar? (y/N):", 0, 0)) {
        log("Instalación cancelada por el usuario.");
        return 2;
    }

    if (cfg.mode == MODE_NEW) {
        log(COLOR_GREEN "[1/3] Preparando tabla de particiones..." COLOR_RESET);
        int table_rc = (cfg.table == TABLE_GPT)
                       ? write_gpt_partition_table(cfg.disk_path, parts, part_count, cfg.dry_run)
                       : write_mbr_partition_table(cfg.disk_path, parts, part_count, cfg.dry_run);
        if (table_rc != 0) {
            return 1;
        }
        trigger_partition_rescan(cfg.disk_path);

        char efi_dev[160], root_dev[160];
        if (partition_dev_path(cfg.disk_path, 1, efi_dev, sizeof(efi_dev)) != 0 ||
            partition_dev_path(cfg.disk_path, 2, root_dev, sizeof(root_dev)) != 0) {
            log(COLOR_RED COLOR_BOLD "ERROR: No se pudo construir la ruta de las particiones." COLOR_RESET);
            return 1;
        }

        int total_steps = (cfg.layout == LAYOUT_ADVANCED) ? 5 : 3;
        (void)total_steps;

        {
            char efi_label[16];
            part1_size_label(efi_label, sizeof(efi_label));
            log(COLOR_GREEN "[2/%d] Escribiendo partición EFI (%s)..." COLOR_RESET,
                (cfg.layout == LAYOUT_ADVANCED) ? 5 : 3, efi_label);
        }
        if (verify_efi_image_fits_partition(&parts[0]) != 0) {
            return 1;
        }
        if (write_partition_image_with_retry(efi_dev, &parts[0], cfg.dry_run) != 0) {
            return 1;
        }
        if (verify_partition_write(efi_dev, &parts[0], cfg.dry_run) != 0) {
            return 1;
        }

        log(COLOR_GREEN "[3/%d] Escribiendo partición ROOT..." COLOR_RESET,
            (cfg.layout == LAYOUT_ADVANCED) ? 5 : 3);
        if (write_partition_image_with_retry(root_dev, &parts[1], cfg.dry_run) != 0) {
            return 1;
        }
        if (verify_partition_write(root_dev, &parts[1], cfg.dry_run) != 0) {
            return 1;
        }

        /* ROOT es btrfs: el kernel lo expandirá a toda la partición en el
         * primer montaje; aquí sólo se sincroniza lo escrito. */
        {
            char grow_root_dev[160];
            if (partition_dev_path(cfg.disk_path, 2, grow_root_dev, sizeof(grow_root_dev)) == 0) {
                resize_root_to_partition(grow_root_dev, cfg.dry_run);
            }
        }

        if (write_fstab_to_root(cfg.disk_path, parts, part_count, cfg.layout, cfg.dry_run) != 0) {
            return 1;
        }

        /* Fija ROOT=<part2> en la cmdline de rboot.conf de la EFI (no fatal). */
        {
            patch_rboot_root_on_efi(efi_dev, root_dev, cfg.dry_run);
        }
    } else {
        if (run_upgrade(cfg.disk_path, cfg.dry_run) != 0) {
            return 1;
        }
    }

    sync();

    struct timespec end_time;
    clock_gettime(CLOCK_MONOTONIC, &end_time);
    time_t elapsed = (time_t)(end_time.tv_sec - start_time.tv_sec);
    int hours = (int)(elapsed / 3600);
    int minutes = (int)((elapsed % 3600) / 60);
    int seconds = (int)(elapsed % 60);

    log(COLOR_GREEN COLOR_BOLD "========================================================" COLOR_RESET);
    if (cfg.mode == MODE_UPGRADE) {
        log(COLOR_GREEN COLOR_BOLD " *         ACTUALIZACIÓN COMPLETADA CON ÉXITO         *" COLOR_RESET);
        log(COLOR_GREEN COLOR_BOLD "========================================================" COLOR_RESET);
        log("Eclipse OS se ha actualizado correctamente en %s.", cfg.disk_path);
        log("El resto del sistema puede actualizarse con apk (p. ej. apk update && apk upgrade).");
    } else {
        log(COLOR_GREEN COLOR_BOLD " *          INSTALACIÓN COMPLETADA CON ÉXITO          *" COLOR_RESET);
        log(COLOR_GREEN COLOR_BOLD "========================================================" COLOR_RESET);
        log("Eclipse OS se ha instalado correctamente en %s.", cfg.disk_path);
    }
    log("Tiempo total transcurrido: %02d:%02d:%02d", hours, minutes, seconds);

    return 0;
}

int struct_file_exists(const char *path) {
    struct stat st;
    return stat(path, &st) == 0;
}

static const char *resolve_image_path(const char *path) {
    static char resolved[256];
    struct stat st;
    if (strncmp(path, "/boot/", 6) == 0) {
        snprintf(resolved, sizeof(resolved), "ignored/target/%s", path + 6);
        if (stat(resolved, &st) == 0) {
            return strdup(resolved);
        }
        snprintf(resolved, sizeof(resolved), "rootfs/x86_64%s", path);
        if (stat(resolved, &st) == 0) {
            return strdup(resolved);
        }
        snprintf(resolved, sizeof(resolved), "boot/%s", path + 6);
        if (stat(resolved, &st) == 0) {
            return strdup(resolved);
        }
    }
    return path;
}
