#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <inttypes.h>
#include <string.h>
#include <strings.h>
#include <stdarg.h>
#include <unistd.h>
#include <errno.h>
#include <ctype.h>
#include <time.h>
#include <fcntl.h>
#include <dirent.h>
#include <termios.h>
#include <limits.h>
#include <stdbool.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <sys/file.h>

#define COLOR_RESET   "\x1b[0m"
#define COLOR_BOLD    "\x1b[1m"
#define COLOR_RED     "\x1b[31m"
#define COLOR_GREEN   "\x1b[32m"
#define COLOR_YELLOW  "\x1b[33m"
#define COLOR_BLUE    "\x1b[34m"
#define COLOR_CYAN    "\x1b[36m"
#define COLOR_WHITE   "\x1b[37m"

#define USERS_FILE "/etc/eclipse-users"
#define SHADOW_FILE "/etc/eclipse-shadow"
#define GROUPS_FILE "/etc/eclipse-groups"

#define MAX_USERS 1024
#define MAX_GROUPS 256
#define MAX_SHADOWS 1024
#define MAX_LINE 4096
#define MAX_USERNAME 32
#define MAX_FULLNAME 64
#define MAX_BIRTHDATE 10
#define MAX_GROUP_CSV 512
#define MAX_HOME 256
#define MAX_SHELL 128
#define MAX_HASH 256
#define ARGON2_VERSION 0x13U
#define ARGON2_TYPE_ID 2U
#define ARGON2_SYNC_POINTS 4U
#define ARGON2_BLOCK_SIZE 1024U
#define ARGON2_QWORDS_IN_BLOCK 128U
#define ARGON2_SALT_LEN 16U
#define ARGON2_HASH_LEN 32U
#define ARGON2_M_COST 65536U
#define ARGON2_T_COST 3U
#define ARGON2_PARALLELISM 1U

struct user_entry {
    char username[MAX_USERNAME + 1];
    uint32_t uid;
    uint32_t primary_gid;
    char fullname[MAX_FULLNAME + 1];
    char birthdate[MAX_BIRTHDATE + 1];
    char homedir[MAX_HOME];
    char shell[MAX_SHELL];
    char resource_groups[MAX_GROUP_CSV];
};

struct shadow_entry {
    char username[MAX_USERNAME + 1];
    char hash[MAX_HASH];
    long last_change_days;
    long min_days;
    long max_days;
    long warn_days;
};

struct group_entry {
    char name[MAX_USERNAME + 1];
    uint32_t gid;
    char description[160];
    char members[MAX_GROUP_CSV];
};

struct options {
    const char *command;
    char user[MAX_USERNAME + 1];
    char fullname[MAX_FULLNAME + 1];
    char birthdate[MAX_BIRTHDATE + 1];
    char groups[MAX_GROUP_CSV];
    char add_groups[MAX_GROUP_CSV];
    char del_groups[MAX_GROUP_CSV];
    char home[MAX_HOME];
    char shell[MAX_SHELL];
    char password[256];
    uint32_t uid;
    int uid_set;
    int remove_home;
};

struct db_locks {
    int users_fd;
    int shadow_fd;
    int groups_fd;
};

struct predefined_group {
    const char *name;
    uint32_t gid;
    const char *description;
    const char *members;
};

struct blake2b_state {
    uint64_t h[8];
    uint64_t t[2];
    uint64_t f[2];
    uint8_t buf[128];
    size_t buflen;
    size_t outlen;
};

typedef struct {
    uint64_t v[ARGON2_QWORDS_IN_BLOCK];
} argon2_block;

static const struct predefined_group g_default_groups[] = {
    {"root", 0, "Super-usuario del sistema", "root"},
    {"users", 100, "Usuarios regulares del sistema", ""},
    {"audio", 200, "Acceso al sistema de audio y sonido", ""},
    {"internet", 201, "Acceso a la red e internet", ""},
    {"video", 202, "Acceso a dispositivos de vídeo y pantalla", ""},
    {"usb", 203, "Acceso a dispositivos USB", ""},
    {"storage", 204, "Acceso a dispositivos de almacenamiento", ""},
    {"camera", 205, "Acceso a cámara y captura de vídeo", ""},
    {"bluetooth", 206, "Acceso a hardware Bluetooth", ""},
    {"input", 207, "Acceso a dispositivos de entrada (teclado, ratón)", ""},
    {"power", 208, "Acceso a gestión de energía (apagar, suspender)", ""},
    {"print", 209, "Acceso al sistema de impresión", ""},
    {"wheel", 210, "Escalada de privilegios administrativos", "root"},
    {"games", 211, "Acceso a recursos de juegos", ""},
    {"crypto", 212, "Acceso a aceleradores hardware de criptografía", ""},
    {"display", 213, "Acceso al servidor de pantalla (Wayland/X11)", ""}
};

static const uint64_t blake2b_iv[8] = {
    UINT64_C(0x6a09e667f3bcc908), UINT64_C(0xbb67ae8584caa73b),
    UINT64_C(0x3c6ef372fe94f82b), UINT64_C(0xa54ff53a5f1d36f1),
    UINT64_C(0x510e527fade682d1), UINT64_C(0x9b05688c2b3e6c1f),
    UINT64_C(0x1f83d9abfb41bd6b), UINT64_C(0x5be0cd19137e2179)
};

static const uint8_t blake2b_sigma[12][16] = {
    { 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,10,11,12,13,14,15 },
    {14,10, 4, 8, 9,15,13, 6, 1,12, 0, 2,11, 7, 5, 3 },
    {11, 8,12, 0, 5, 2,15,13,10,14, 3, 6, 7, 1, 9, 4 },
    { 7, 9, 3, 1,13,12,11,14, 2, 6, 5,10, 4, 0,15, 8 },
    { 9, 0, 5, 7, 2, 4,10,15,14, 1,11,12, 6, 8, 3,13 },
    { 2,12, 6,10, 0,11, 8, 3, 4,13, 7, 5,15,14, 1, 9 },
    {12, 5, 1,15,14,13, 4,10, 0, 7, 6, 3, 9, 2, 8,11 },
    {13,11, 7,14,12, 1, 3, 9, 5, 0,15, 4, 8, 6, 2,10 },
    { 6,15,14, 9,11, 3, 0, 8,12, 2,13, 7, 1, 4,10, 5 },
    {10, 2, 8, 4, 7, 6, 1, 5,15,11, 9,14, 3,12,13, 0 },
    { 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,10,11,12,13,14,15 },
    {14,10, 4, 8, 9,15,13, 6, 1,12, 0, 2,11, 7, 5, 3 }
};

static void print_header(void);
static void print_usage(const char *progname);
static void secure_bzero(void *ptr, size_t len);
static void clear_stdin_until_newline(void);
static void trim_newline(char *s);
static void trim_whitespace(char *s);
static int string_copy(char *dst, size_t dstsz, const char *src);
static void print_error(const char *fmt, ...);
static void print_success(const char *fmt, ...);
static void print_info(const char *fmt, ...);
static int validate_username(const char *username);
static int validate_fullname(const char *fullname);
static int validate_birthdate(const char *birthdate);
static long days_since_epoch(void);
static int ensure_root(const char *command);
static int ensure_parent_directory(const char *path, mode_t mode);
static int prompt_input(const char *label, char *buf, size_t buflen, const char *default_value);
static int read_hidden_line(const char *prompt, char *buf, size_t buflen);
static int prompt_password_twice(char *password, size_t password_len);
static uint32_t next_available_uid(const struct user_entry *users, size_t user_count);
static int csv_contains(const char *csv, const char *item);
static int csv_add(char *csv, size_t csvlen, const char *item);
static void csv_remove(char *csv, const char *item);
static int csv_normalize(char *dst, size_t dstlen, const char *src);
static int write_atomic_text_file(const char *path, const char *content, mode_t mode);
static int lock_db_files(struct db_locks *locks);
static void unlock_db_files(struct db_locks *locks);
static int load_users(struct user_entry *users, size_t *count);
static int load_shadows(struct shadow_entry *shadows, size_t *count);
static int load_groups(struct group_entry *groups, size_t *count);
static int save_users(const struct user_entry *users, size_t count);
static int save_shadows(const struct shadow_entry *shadows, size_t count);
static int save_groups(const struct group_entry *groups, size_t count);
static ssize_t find_user_by_name(const struct user_entry *users, size_t count, const char *username);
static ssize_t find_user_by_uid(const struct user_entry *users, size_t count, uint32_t uid);
static ssize_t find_shadow_by_name(const struct shadow_entry *shadows, size_t count, const char *username);
static ssize_t find_group_by_name(const struct group_entry *groups, size_t count, const char *name);
static void apply_memberships_from_users(struct group_entry *groups, size_t group_count, const struct user_entry *users, size_t user_count);
static int create_home_directory(const char *path, uint32_t uid, uint32_t gid);
static int recursive_delete_path(const char *path);
static int create_password_hash(const char *password, char *encoded, size_t encoded_len);
static int parse_options(int argc, char **argv, struct options *opts);
static int command_init(void);
static int command_useradd(struct options *opts);
static int command_userdel(struct options *opts);
static int command_usermod(struct options *opts);
static int command_passwd(struct options *opts);
static int command_userlist(void);
static int command_grouplist(void);
static int interactive_menu(const char *progname);

static inline uint64_t load64_le(const void *src) {
    const uint8_t *p = (const uint8_t *)src;
    return ((uint64_t)p[0]) |
           ((uint64_t)p[1] << 8) |
           ((uint64_t)p[2] << 16) |
           ((uint64_t)p[3] << 24) |
           ((uint64_t)p[4] << 32) |
           ((uint64_t)p[5] << 40) |
           ((uint64_t)p[6] << 48) |
           ((uint64_t)p[7] << 56);
}

static inline void store64_le(void *dst, uint64_t w) {
    uint8_t *p = (uint8_t *)dst;
    p[0] = (uint8_t)w;
    p[1] = (uint8_t)(w >> 8);
    p[2] = (uint8_t)(w >> 16);
    p[3] = (uint8_t)(w >> 24);
    p[4] = (uint8_t)(w >> 32);
    p[5] = (uint8_t)(w >> 40);
    p[6] = (uint8_t)(w >> 48);
    p[7] = (uint8_t)(w >> 56);
}

static inline void store32_le(void *dst, uint32_t w) {
    uint8_t *p = (uint8_t *)dst;
    p[0] = (uint8_t)w;
    p[1] = (uint8_t)(w >> 8);
    p[2] = (uint8_t)(w >> 16);
    p[3] = (uint8_t)(w >> 24);
}

static inline uint64_t rotr64(uint64_t x, unsigned int n) {
    return (x >> n) | (x << (64U - n));
}

static void secure_bzero(void *ptr, size_t len) {
    volatile uint8_t *p = (volatile uint8_t *)ptr;
    while (len-- > 0U) {
        *p++ = 0U;
    }
    __asm__ __volatile__("" : : : "memory");
}

static void clear_stdin_until_newline(void) {
    int ch;
    do {
        ch = getchar();
    } while (ch != '\n' && ch != EOF);
}

static void trim_newline(char *s) {
    if (s == NULL) {
        return;
    }
    s[strcspn(s, "\r\n")] = '\0';
}

static void trim_whitespace(char *s) {
    size_t len;
    size_t start = 0U;
    if (s == NULL) {
        return;
    }
    len = strlen(s);
    while (start < len && isspace((unsigned char)s[start])) {
        start++;
    }
    if (start > 0U) {
        memmove(s, s + start, len - start + 1U);
    }
    len = strlen(s);
    while (len > 0U && isspace((unsigned char)s[len - 1U])) {
        s[len - 1U] = '\0';
        len--;
    }
}

static int string_copy(char *dst, size_t dstsz, const char *src) {
    size_t len;
    if (dst == NULL || src == NULL || dstsz == 0U) {
        return -1;
    }
    len = strnlen(src, dstsz);
    if (len >= dstsz) {
        return -1;
    }
    memcpy(dst, src, len + 1U);
    return 0;
}

static void vprint_color(const char *color, const char *fmt, va_list ap) {
    fputs(color, stdout);
    vfprintf(stdout, fmt, ap);
    fputs(COLOR_RESET "\n", stdout);
}

static void print_error(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vprint_color(COLOR_RED COLOR_BOLD, fmt, ap);
    va_end(ap);
}

static void print_success(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vprint_color(COLOR_GREEN, fmt, ap);
    va_end(ap);
}

static void print_info(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vprint_color(COLOR_CYAN, fmt, ap);
    va_end(ap);
}

static void print_header(void) {
    printf("\x1b[2J\x1b[H");
    printf(COLOR_CYAN COLOR_BOLD "========================================================\n" COLOR_RESET);
    printf(COLOR_BLUE COLOR_BOLD "        ECLIPSE OS - GESTIÓN DE USUARIOS\n" COLOR_RESET);
    printf(COLOR_CYAN COLOR_BOLD "========================================================\n\n" COLOR_RESET);
}

static void print_usage(const char *progname) {
    print_header();
    printf("Uso: %s [COMANDO] [OPCIONES]\n\n", progname);
    printf("Comandos:\n");
    printf("  init                 Inicializa las bases de datos\n");
    printf("  useradd              Crea un usuario\n");
    printf("  userdel              Elimina un usuario\n");
    printf("  usermod              Modifica un usuario\n");
    printf("  passwd               Cambia una contraseña\n");
    printf("  userlist             Lista usuarios\n");
    printf("  grouplist            Lista grupos de recursos\n");
    printf("  help                 Muestra esta ayuda\n\n");
    printf("Opciones comunes:\n");
    printf("  --user <nombre>\n");
    printf("  --fullname <nombre completo>\n");
    printf("  --uid <n>\n");
    printf("  --birth <YYYY-MM-DD>\n");
    printf("  --groups <g1,g2>\n");
    printf("  --add-groups <g1,g2>\n");
    printf("  --del-groups <g1,g2>\n");
    printf("  --home <ruta>\n");
    printf("  --shell <ruta>\n");
    printf("  --password <clave>\n");
    printf("  --remove-home\n");
}

static int validate_username(const char *username) {
    size_t i;
    size_t len;
    if (username == NULL) {
        return 0;
    }
    len = strnlen(username, MAX_USERNAME + 2U);
    if (len == 0U || len > MAX_USERNAME) {
        return 0;
    }
    for (i = 0U; i < len; ++i) {
        unsigned char ch = (unsigned char)username[i];
        if (!(isalnum(ch) || ch == '_' || ch == '-')) {
            return 0;
        }
    }
    return 1;
}

static int validate_fullname(const char *fullname) {
    size_t len;
    if (fullname == NULL) {
        return 0;
    }
    len = strnlen(fullname, MAX_FULLNAME + 1U);
    return len > 0U && len <= MAX_FULLNAME;
}

static int is_leap_year(int year) {
    return ((year % 4 == 0) && (year % 100 != 0)) || (year % 400 == 0);
}

static int validate_birthdate(const char *birthdate) {
    int year;
    int month;
    int day;
    static const int mdays[] = { 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31 };
    if (birthdate == NULL || strlen(birthdate) != 10U) {
        return 0;
    }
    if (strcmp(birthdate, "0000-00-00") == 0) {
        return 1;
    }
    if (sscanf(birthdate, "%4d-%2d-%2d", &year, &month, &day) != 3) {
        return 0;
    }
    if (month < 1 || month > 12 || day < 1) {
        return 0;
    }
    if (month == 2 && is_leap_year(year)) {
        return day <= 29;
    }
    return day <= mdays[month - 1];
}

static long days_since_epoch(void) {
    time_t now = time(NULL);
    if (now < 0) {
        return 0;
    }
    return (long)(now / 86400);
}

static int ensure_root(const char *command) {
    if (geteuid() != 0) {
        print_error("%s requiere privilegios de root.", command);
        return -1;
    }
    return 0;
}

static int ensure_parent_directory(const char *path, mode_t mode) {
    char copy[PATH_MAX];
    char *slash;
    if (string_copy(copy, sizeof(copy), path) != 0) {
        return -1;
    }
    slash = strrchr(copy, '/');
    if (slash == NULL) {
        return 0;
    }
    *slash = '\0';
    if (copy[0] == '\0') {
        return 0;
    }
    if (mkdir(copy, mode) == 0 || errno == EEXIST) {
        return 0;
    }
    return -1;
}

static int prompt_input(const char *label, char *buf, size_t buflen, const char *default_value) {
    if (buf == NULL || buflen == 0U) {
        return -1;
    }
    if (default_value != NULL && default_value[0] != '\0') {
        printf("%s [%s]: ", label, default_value);
    } else {
        printf("%s: ", label);
    }
    fflush(stdout);
    if (fgets(buf, (int)buflen, stdin) == NULL) {
        return -1;
    }
    trim_newline(buf);
    trim_whitespace(buf);
    if (buf[0] == '\0' && default_value != NULL) {
        return string_copy(buf, buflen, default_value);
    }
    return 0;
}

static int read_hidden_line(const char *prompt, char *buf, size_t buflen) {
    FILE *tty;
    int fd;
    struct termios oldt;
    struct termios newt;
    if (buf == NULL || buflen == 0U) {
        return -1;
    }
    tty = fopen("/dev/tty", "r+");
    if (tty == NULL) {
        tty = stdin;
    }
    fd = fileno(tty);
    if (isatty(fd) != 0) {
        if (tcgetattr(fd, &oldt) != 0) {
            if (tty != stdin) {
                fclose(tty);
            }
            return -1;
        }
        newt = oldt;
        newt.c_lflag &= (tcflag_t)~ECHO;
        if (tcsetattr(fd, TCSAFLUSH, &newt) != 0) {
            if (tty != stdin) {
                fclose(tty);
            }
            return -1;
        }
    }
    fprintf(tty, "%s", prompt);
    fflush(tty);
    if (fgets(buf, (int)buflen, tty) == NULL) {
        if (isatty(fd) != 0) {
            tcsetattr(fd, TCSAFLUSH, &oldt);
            fprintf(tty, "\n");
        }
        if (tty != stdin) {
            fclose(tty);
        }
        return -1;
    }
    if (isatty(fd) != 0) {
        tcsetattr(fd, TCSAFLUSH, &oldt);
        fprintf(tty, "\n");
        fflush(tty);
    }
    trim_newline(buf);
    if (tty != stdin) {
        fclose(tty);
    }
    return 0;
}

static int prompt_password_twice(char *password, size_t password_len) {
    char first[256];
    char second[256];
    if (read_hidden_line("Contraseña: ", first, sizeof(first)) != 0) {
        print_error("No se pudo leer la contraseña.");
        return -1;
    }
    if (read_hidden_line("Confirmar contraseña: ", second, sizeof(second)) != 0) {
        secure_bzero(first, sizeof(first));
        print_error("No se pudo confirmar la contraseña.");
        return -1;
    }
    if (strcmp(first, second) != 0) {
        secure_bzero(first, sizeof(first));
        secure_bzero(second, sizeof(second));
        print_error("Las contraseñas no coinciden.");
        return -1;
    }
    if (first[0] == '\0') {
        secure_bzero(first, sizeof(first));
        secure_bzero(second, sizeof(second));
        print_error("La contraseña no puede estar vacía.");
        return -1;
    }
    if (string_copy(password, password_len, first) != 0) {
        secure_bzero(first, sizeof(first));
        secure_bzero(second, sizeof(second));
        print_error("La contraseña es demasiado larga.");
        return -1;
    }
    secure_bzero(first, sizeof(first));
    secure_bzero(second, sizeof(second));
    return 0;
}

static uint32_t next_available_uid(const struct user_entry *users, size_t user_count) {
    uint32_t next_uid = 1000U;
    size_t i;
    for (i = 0U; i < user_count; ++i) {
        if (users[i].uid >= next_uid) {
            next_uid = users[i].uid + 1U;
        }
    }
    if (next_uid < 1000U) {
        next_uid = 1000U;
    }
    return next_uid;
}

static int csv_contains(const char *csv, const char *item) {
    char copy[MAX_GROUP_CSV];
    char *saveptr = NULL;
    char *tok;
    if (csv == NULL || item == NULL || item[0] == '\0') {
        return 0;
    }
    if (string_copy(copy, sizeof(copy), csv) != 0) {
        return 0;
    }
    tok = strtok_r(copy, ",", &saveptr);
    while (tok != NULL) {
        trim_whitespace(tok);
        if (strcmp(tok, item) == 0) {
            return 1;
        }
        tok = strtok_r(NULL, ",", &saveptr);
    }
    return 0;
}

static int csv_add(char *csv, size_t csvlen, const char *item) {
    size_t used;
    if (csv == NULL || item == NULL || item[0] == '\0') {
        return -1;
    }
    if (csv_contains(csv, item)) {
        return 0;
    }
    used = strlen(csv);
    if (used == 0U) {
        return string_copy(csv, csvlen, item);
    }
    if (used + 1U + strlen(item) + 1U > csvlen) {
        return -1;
    }
    csv[used] = ',';
    csv[used + 1U] = '\0';
    strcat(csv, item);
    return 0;
}

static void csv_remove(char *csv, const char *item) {
    char copy[MAX_GROUP_CSV];
    char result[MAX_GROUP_CSV] = "";
    char *saveptr = NULL;
    char *tok;
    if (csv == NULL || item == NULL) {
        return;
    }
    if (string_copy(copy, sizeof(copy), csv) != 0) {
        return;
    }
    tok = strtok_r(copy, ",", &saveptr);
    while (tok != NULL) {
        trim_whitespace(tok);
        if (tok[0] != '\0' && strcmp(tok, item) != 0) {
            (void)csv_add(result, sizeof(result), tok);
        }
        tok = strtok_r(NULL, ",", &saveptr);
    }
    (void)string_copy(csv, MAX_GROUP_CSV, result);
}

static int csv_normalize(char *dst, size_t dstlen, const char *src) {
    char copy[MAX_GROUP_CSV];
    char result[MAX_GROUP_CSV] = "";
    char *saveptr = NULL;
    char *tok;
    if (dst == NULL || dstlen == 0U) {
        return -1;
    }
    dst[0] = '\0';
    if (src == NULL || src[0] == '\0') {
        return 0;
    }
    if (string_copy(copy, sizeof(copy), src) != 0) {
        return -1;
    }
    tok = strtok_r(copy, ",", &saveptr);
    while (tok != NULL) {
        trim_whitespace(tok);
        if (tok[0] != '\0') {
            if (!validate_username(tok)) {
                return -1;
            }
            if (csv_add(result, sizeof(result), tok) != 0) {
                return -1;
            }
        }
        tok = strtok_r(NULL, ",", &saveptr);
    }
    return string_copy(dst, dstlen, result);
}

static int write_atomic_text_file(const char *path, const char *content, mode_t mode) {
    char tmp_path[PATH_MAX];
    int fd;
    ssize_t wrote;
    size_t len;
    if (snprintf(tmp_path, sizeof(tmp_path), "%s.tmp.%ld", path, (long)getpid()) >= (int)sizeof(tmp_path)) {
        return -1;
    }
    if (ensure_parent_directory(path, 0755) != 0) {
        return -1;
    }
    fd = open(tmp_path, O_WRONLY | O_CREAT | O_TRUNC, mode);
    if (fd < 0) {
        return -1;
    }
    if (fchmod(fd, mode) != 0) {
        close(fd);
        unlink(tmp_path);
        return -1;
    }
    len = strlen(content);
    wrote = write(fd, content, len);
    if (wrote < 0 || (size_t)wrote != len) {
        close(fd);
        unlink(tmp_path);
        return -1;
    }
    if (fsync(fd) != 0) {
        close(fd);
        unlink(tmp_path);
        return -1;
    }
    if (close(fd) != 0) {
        unlink(tmp_path);
        return -1;
    }
    if (rename(tmp_path, path) != 0) {
        unlink(tmp_path);
        return -1;
    }
    return 0;
}

static int open_and_lock_file(const char *path, mode_t mode) {
    int fd;
    if (ensure_parent_directory(path, 0755) != 0) {
        return -1;
    }
    fd = open(path, O_RDWR | O_CREAT, mode);
    if (fd < 0) {
        return -1;
    }
    if (flock(fd, LOCK_EX) != 0) {
        close(fd);
        return -1;
    }
    return fd;
}

static int lock_db_files(struct db_locks *locks) {
    if (locks == NULL) {
        return -1;
    }
    locks->users_fd = -1;
    locks->shadow_fd = -1;
    locks->groups_fd = -1;
    locks->users_fd = open_and_lock_file(USERS_FILE, 0600);
    if (locks->users_fd < 0) {
        return -1;
    }
    locks->shadow_fd = open_and_lock_file(SHADOW_FILE, 0400);
    if (locks->shadow_fd < 0) {
        unlock_db_files(locks);
        return -1;
    }
    locks->groups_fd = open_and_lock_file(GROUPS_FILE, 0644);
    if (locks->groups_fd < 0) {
        unlock_db_files(locks);
        return -1;
    }
    return 0;
}

static void unlock_db_files(struct db_locks *locks) {
    if (locks == NULL) {
        return;
    }
    if (locks->groups_fd >= 0) {
        flock(locks->groups_fd, LOCK_UN);
        close(locks->groups_fd);
        locks->groups_fd = -1;
    }
    if (locks->shadow_fd >= 0) {
        flock(locks->shadow_fd, LOCK_UN);
        close(locks->shadow_fd);
        locks->shadow_fd = -1;
    }
    if (locks->users_fd >= 0) {
        flock(locks->users_fd, LOCK_UN);
        close(locks->users_fd);
        locks->users_fd = -1;
    }
}

static int split_fields_preserve_empty(char *line, char **fields, size_t expected) {
    size_t count = 0U;
    char *start = line;
    char *p = line;
    while (count < expected) {
        if (*p == ':' || *p == '\0') {
            fields[count++] = start;
            if (*p == '\0') {
                break;
            }
            *p = '\0';
            start = p + 1;
        }
        p++;
    }
    while (count < expected) {
        fields[count++] = start;
        start += strlen(start);
    }
    return (int)count;
}

static int load_users(struct user_entry *users, size_t *count) {
    FILE *fp;
    char line[MAX_LINE];
    size_t used = 0U;
    if (users == NULL || count == NULL) {
        return -1;
    }
    *count = 0U;
    fp = fopen(USERS_FILE, "r");
    if (fp == NULL) {
        if (errno == ENOENT) {
            return 0;
        }
        return -1;
    }
    while (fgets(line, sizeof(line), fp) != NULL) {
        char *fields[8];
        trim_newline(line);
        if (line[0] == '\0' || line[0] == '#') {
            continue;
        }
        if (split_fields_preserve_empty(line, fields, 8U) != 8 || used >= MAX_USERS) {
            fclose(fp);
            return -1;
        }
        if (string_copy(users[used].username, sizeof(users[used].username), fields[0]) != 0 ||
            string_copy(users[used].fullname, sizeof(users[used].fullname), fields[3]) != 0 ||
            string_copy(users[used].birthdate, sizeof(users[used].birthdate), fields[4]) != 0 ||
            string_copy(users[used].homedir, sizeof(users[used].homedir), fields[5]) != 0 ||
            string_copy(users[used].shell, sizeof(users[used].shell), fields[6]) != 0 ||
            string_copy(users[used].resource_groups, sizeof(users[used].resource_groups), fields[7]) != 0) {
            fclose(fp);
            return -1;
        }
        users[used].uid = (uint32_t)strtoul(fields[1], NULL, 10);
        users[used].primary_gid = (uint32_t)strtoul(fields[2], NULL, 10);
        used++;
    }
    fclose(fp);
    *count = used;
    return 0;
}

static int load_shadows(struct shadow_entry *shadows, size_t *count) {
    FILE *fp;
    char line[MAX_LINE];
    size_t used = 0U;
    if (shadows == NULL || count == NULL) {
        return -1;
    }
    *count = 0U;
    fp = fopen(SHADOW_FILE, "r");
    if (fp == NULL) {
        if (errno == ENOENT) {
            return 0;
        }
        return -1;
    }
    while (fgets(line, sizeof(line), fp) != NULL) {
        char *fields[6];
        trim_newline(line);
        if (line[0] == '\0' || line[0] == '#') {
            continue;
        }
        if (split_fields_preserve_empty(line, fields, 6U) != 6 || used >= MAX_SHADOWS) {
            fclose(fp);
            return -1;
        }
        if (string_copy(shadows[used].username, sizeof(shadows[used].username), fields[0]) != 0 ||
            string_copy(shadows[used].hash, sizeof(shadows[used].hash), fields[1]) != 0) {
            fclose(fp);
            return -1;
        }
        shadows[used].last_change_days = strtol(fields[2], NULL, 10);
        shadows[used].min_days = strtol(fields[3], NULL, 10);
        shadows[used].max_days = strtol(fields[4], NULL, 10);
        shadows[used].warn_days = strtol(fields[5], NULL, 10);
        used++;
    }
    fclose(fp);
    *count = used;
    return 0;
}

static int load_groups(struct group_entry *groups, size_t *count) {
    FILE *fp;
    char line[MAX_LINE];
    size_t used = 0U;
    if (groups == NULL || count == NULL) {
        return -1;
    }
    *count = 0U;
    fp = fopen(GROUPS_FILE, "r");
    if (fp == NULL) {
        if (errno == ENOENT) {
            return 0;
        }
        return -1;
    }
    while (fgets(line, sizeof(line), fp) != NULL) {
        char *fields[4];
        trim_newline(line);
        if (line[0] == '\0' || line[0] == '#') {
            continue;
        }
        if (split_fields_preserve_empty(line, fields, 4U) < 3 || used >= MAX_GROUPS) {
            fclose(fp);
            return -1;
        }
        if (string_copy(groups[used].name, sizeof(groups[used].name), fields[0]) != 0 ||
            string_copy(groups[used].description, sizeof(groups[used].description), fields[2]) != 0 ||
            string_copy(groups[used].members, sizeof(groups[used].members), fields[3]) != 0) {
            fclose(fp);
            return -1;
        }
        groups[used].gid = (uint32_t)strtoul(fields[1], NULL, 10);
        used++;
    }
    fclose(fp);
    *count = used;
    return 0;
}

static int save_users(const struct user_entry *users, size_t count) {
    char *buffer;
    size_t i;
    size_t offset = 0U;
    size_t cap = (count + 4U) * 512U;
    buffer = (char *)calloc(cap, 1U);
    if (buffer == NULL) {
        return -1;
    }
    for (i = 0U; i < count; ++i) {
        int wrote = snprintf(buffer + offset, cap - offset, "%s:%" PRIu32 ":%" PRIu32 ":%s:%s:%s:%s:%s\n",
                             users[i].username, users[i].uid, users[i].primary_gid,
                             users[i].fullname, users[i].birthdate, users[i].homedir,
                             users[i].shell, users[i].resource_groups);
        if (wrote < 0 || (size_t)wrote >= cap - offset) {
            free(buffer);
            return -1;
        }
        offset += (size_t)wrote;
    }
    if (write_atomic_text_file(USERS_FILE, buffer, 0600) != 0) {
        free(buffer);
        return -1;
    }
    free(buffer);
    return 0;
}

static int save_shadows(const struct shadow_entry *shadows, size_t count) {
    char *buffer;
    size_t i;
    size_t offset = 0U;
    size_t cap = (count + 4U) * 512U;
    buffer = (char *)calloc(cap, 1U);
    if (buffer == NULL) {
        return -1;
    }
    for (i = 0U; i < count; ++i) {
        int wrote = snprintf(buffer + offset, cap - offset, "%s:%s:%ld:%ld:%ld:%ld\n",
                             shadows[i].username, shadows[i].hash, shadows[i].last_change_days,
                             shadows[i].min_days, shadows[i].max_days, shadows[i].warn_days);
        if (wrote < 0 || (size_t)wrote >= cap - offset) {
            free(buffer);
            return -1;
        }
        offset += (size_t)wrote;
    }
    if (write_atomic_text_file(SHADOW_FILE, buffer, 0400) != 0) {
        free(buffer);
        return -1;
    }
    free(buffer);
    return 0;
}

static int save_groups(const struct group_entry *groups, size_t count) {
    char *buffer;
    size_t i;
    size_t offset = 0U;
    size_t cap = (count + 4U) * 512U;
    buffer = (char *)calloc(cap, 1U);
    if (buffer == NULL) {
        return -1;
    }
    for (i = 0U; i < count; ++i) {
        int wrote = snprintf(buffer + offset, cap - offset, "%s:%" PRIu32 ":%s:%s\n",
                             groups[i].name, groups[i].gid, groups[i].description, groups[i].members);
        if (wrote < 0 || (size_t)wrote >= cap - offset) {
            free(buffer);
            return -1;
        }
        offset += (size_t)wrote;
    }
    if (write_atomic_text_file(GROUPS_FILE, buffer, 0644) != 0) {
        free(buffer);
        return -1;
    }
    free(buffer);
    return 0;
}

static ssize_t find_user_by_name(const struct user_entry *users, size_t count, const char *username) {
    size_t i;
    for (i = 0U; i < count; ++i) {
        if (strcmp(users[i].username, username) == 0) {
            return (ssize_t)i;
        }
    }
    return -1;
}

static ssize_t find_user_by_uid(const struct user_entry *users, size_t count, uint32_t uid) {
    size_t i;
    for (i = 0U; i < count; ++i) {
        if (users[i].uid == uid) {
            return (ssize_t)i;
        }
    }
    return -1;
}

static ssize_t find_shadow_by_name(const struct shadow_entry *shadows, size_t count, const char *username) {
    size_t i;
    for (i = 0U; i < count; ++i) {
        if (strcmp(shadows[i].username, username) == 0) {
            return (ssize_t)i;
        }
    }
    return -1;
}

static ssize_t find_group_by_name(const struct group_entry *groups, size_t count, const char *name) {
    size_t i;
    for (i = 0U; i < count; ++i) {
        if (strcmp(groups[i].name, name) == 0) {
            return (ssize_t)i;
        }
    }
    return -1;
}

static void apply_memberships_from_users(struct group_entry *groups, size_t group_count, const struct user_entry *users, size_t user_count) {
    size_t i;
    size_t j;
    for (i = 0U; i < group_count; ++i) {
        groups[i].members[0] = '\0';
    }
    for (j = 0U; j < user_count; ++j) {
        char copy[MAX_GROUP_CSV];
        char *saveptr = NULL;
        char *tok;
        if (strcmp(users[j].username, "root") == 0) {
            ssize_t root_group = find_group_by_name(groups, group_count, "root");
            if (root_group >= 0) {
                (void)csv_add(groups[(size_t)root_group].members, sizeof(groups[(size_t)root_group].members), users[j].username);
            }
        }
        if (string_copy(copy, sizeof(copy), users[j].resource_groups) != 0) {
            continue;
        }
        tok = strtok_r(copy, ",", &saveptr);
        while (tok != NULL) {
            ssize_t idx;
            trim_whitespace(tok);
            idx = find_group_by_name(groups, group_count, tok);
            if (idx >= 0) {
                (void)csv_add(groups[(size_t)idx].members, sizeof(groups[(size_t)idx].members), users[j].username);
            }
            tok = strtok_r(NULL, ",", &saveptr);
        }
    }
    {
        ssize_t root_group = find_group_by_name(groups, group_count, "root");
        if (root_group >= 0) {
            (void)csv_add(groups[(size_t)root_group].members, sizeof(groups[(size_t)root_group].members), "root");
        }
    }
}

static int create_home_directory(const char *path, uint32_t uid, uint32_t gid) {
    if (mkdir(path, 0700) != 0 && errno != EEXIST) {
        return -1;
    }
    if (chmod(path, 0700) != 0) {
        return -1;
    }
    if (chown(path, (uid_t)uid, (gid_t)gid) != 0) {
        return -1;
    }
    return 0;
}

static int recursive_delete_path(const char *path) {
    struct stat st;
    if (lstat(path, &st) != 0) {
        return -1;
    }
    if (S_ISDIR(st.st_mode)) {
        DIR *dir = opendir(path);
        struct dirent *ent;
        if (dir == NULL) {
            return -1;
        }
        while ((ent = readdir(dir)) != NULL) {
            char child[PATH_MAX];
            if (strcmp(ent->d_name, ".") == 0 || strcmp(ent->d_name, "..") == 0) {
                continue;
            }
            if (snprintf(child, sizeof(child), "%s/%s", path, ent->d_name) >= (int)sizeof(child)) {
                closedir(dir);
                return -1;
            }
            if (recursive_delete_path(child) != 0) {
                closedir(dir);
                return -1;
            }
        }
        closedir(dir);
        return rmdir(path);
    }
    return unlink(path);
}

static const char b64_table[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

static int base64_encode(char *out, size_t outlen, const uint8_t *in, size_t inlen) {
    size_t i = 0U;
    size_t j = 0U;
    while (i + 3U <= inlen) {
        uint32_t v = ((uint32_t)in[i] << 16) | ((uint32_t)in[i + 1U] << 8) | (uint32_t)in[i + 2U];
        if (j + 4U >= outlen) {
            return -1;
        }
        out[j++] = b64_table[(v >> 18) & 0x3fU];
        out[j++] = b64_table[(v >> 12) & 0x3fU];
        out[j++] = b64_table[(v >> 6) & 0x3fU];
        out[j++] = b64_table[v & 0x3fU];
        i += 3U;
    }
    if (i < inlen) {
        uint32_t v = (uint32_t)in[i] << 16;
        if (i + 1U < inlen) {
            v |= (uint32_t)in[i + 1U] << 8;
        }
        if (j + 4U >= outlen) {
            return -1;
        }
        out[j++] = b64_table[(v >> 18) & 0x3fU];
        out[j++] = b64_table[(v >> 12) & 0x3fU];
        if (i + 1U < inlen) {
            out[j++] = b64_table[(v >> 6) & 0x3fU];
        }
    }
    if (j >= outlen) {
        return -1;
    }
    out[j] = '\0';
    return (int)j;
}

static int b64_rev(int ch) {
    const char *p = strchr(b64_table, ch);
    if (p == NULL) {
        return -1;
    }
    return (int)(p - b64_table);
}

static int base64_decode(uint8_t *out, size_t outlen, const char *in, size_t inlen) {
    size_t i = 0U;
    size_t j = 0U;
    while (i < inlen) {
        int c0;
        int c1;
        int c2 = -1;
        int c3 = -1;
        uint32_t v;
        if (i + 1U >= inlen) {
            return -1;
        }
        c0 = b64_rev((unsigned char)in[i++]);
        c1 = b64_rev((unsigned char)in[i++]);
        if (c0 < 0 || c1 < 0) {
            return -1;
        }
        if (i < inlen) {
            c2 = b64_rev((unsigned char)in[i]);
            if (c2 >= 0) {
                i++;
            }
        }
        if (i < inlen) {
            c3 = b64_rev((unsigned char)in[i]);
            if (c3 >= 0) {
                i++;
            }
        }
        v = ((uint32_t)c0 << 18) | ((uint32_t)c1 << 12);
        if (c2 >= 0) {
            v |= (uint32_t)c2 << 6;
        }
        if (c3 >= 0) {
            v |= (uint32_t)c3;
        }
        if (j >= outlen) {
            return -1;
        }
        out[j++] = (uint8_t)(v >> 16);
        if (c2 >= 0) {
            if (j >= outlen) {
                return -1;
            }
            out[j++] = (uint8_t)(v >> 8);
            if (c3 >= 0) {
                if (j >= outlen) {
                    return -1;
                }
                out[j++] = (uint8_t)v;
            }
        }
    }
    return (int)j;
}

#define B2B_G(a, b, c, d, x, y) \
    do { \
        a = a + b + (x); \
        d = rotr64(d ^ a, 32); \
        c = c + d; \
        b = rotr64(b ^ c, 24); \
        a = a + b + (y); \
        d = rotr64(d ^ a, 16); \
        c = c + d; \
        b = rotr64(b ^ c, 63); \
    } while (0)

static void blake2b_compress(struct blake2b_state *S, const uint8_t block[128], int last) {
    uint64_t m[16];
    uint64_t v[16];
    size_t i;
    for (i = 0U; i < 16U; ++i) {
        m[i] = load64_le(block + i * 8U);
    }
    for (i = 0U; i < 8U; ++i) {
        v[i] = S->h[i];
        v[i + 8U] = blake2b_iv[i];
    }
    v[12] ^= S->t[0];
    v[13] ^= S->t[1];
    if (last != 0) {
        v[14] = ~v[14];
    }
    for (i = 0U; i < 12U; ++i) {
        B2B_G(v[0], v[4], v[8], v[12], m[blake2b_sigma[i][0]], m[blake2b_sigma[i][1]]);
        B2B_G(v[1], v[5], v[9], v[13], m[blake2b_sigma[i][2]], m[blake2b_sigma[i][3]]);
        B2B_G(v[2], v[6], v[10], v[14], m[blake2b_sigma[i][4]], m[blake2b_sigma[i][5]]);
        B2B_G(v[3], v[7], v[11], v[15], m[blake2b_sigma[i][6]], m[blake2b_sigma[i][7]]);
        B2B_G(v[0], v[5], v[10], v[15], m[blake2b_sigma[i][8]], m[blake2b_sigma[i][9]]);
        B2B_G(v[1], v[6], v[11], v[12], m[blake2b_sigma[i][10]], m[blake2b_sigma[i][11]]);
        B2B_G(v[2], v[7], v[8], v[13], m[blake2b_sigma[i][12]], m[blake2b_sigma[i][13]]);
        B2B_G(v[3], v[4], v[9], v[14], m[blake2b_sigma[i][14]], m[blake2b_sigma[i][15]]);
    }
    for (i = 0U; i < 8U; ++i) {
        S->h[i] ^= v[i] ^ v[i + 8U];
    }
}

static int blake2b_init(struct blake2b_state *S, size_t outlen) {
    size_t i;
    if (S == NULL || outlen == 0U || outlen > 64U) {
        return -1;
    }
    memset(S, 0, sizeof(*S));
    for (i = 0U; i < 8U; ++i) {
        S->h[i] = blake2b_iv[i];
    }
    S->h[0] ^= UINT64_C(0x01010000) ^ (uint64_t)outlen;
    S->outlen = outlen;
    return 0;
}

static void blake2b_increment_counter(struct blake2b_state *S, uint64_t inc) {
    S->t[0] += inc;
    if (S->t[0] < inc) {
        S->t[1]++;
    }
}

static int blake2b_update(struct blake2b_state *S, const void *pin, size_t inlen) {
    const uint8_t *in = (const uint8_t *)pin;
    size_t left = S->buflen;
    size_t fill = 128U - left;
    if (inlen > fill) {
        S->buflen = 0U;
        memcpy(S->buf + left, in, fill);
        blake2b_increment_counter(S, 128U);
        blake2b_compress(S, S->buf, 0);
        in += fill;
        inlen -= fill;
        while (inlen > 128U) {
            blake2b_increment_counter(S, 128U);
            blake2b_compress(S, in, 0);
            in += 128U;
            inlen -= 128U;
        }
        left = 0U;
    }
    memcpy(S->buf + left, in, inlen);
    S->buflen = left + inlen;
    return 0;
}

static int blake2b_final(struct blake2b_state *S, void *out, size_t outlen) {
    size_t i;
    uint8_t buffer[64];
    if (outlen != S->outlen) {
        return -1;
    }
    blake2b_increment_counter(S, (uint64_t)S->buflen);
    memset(S->buf + S->buflen, 0, 128U - S->buflen);
    blake2b_compress(S, S->buf, 1);
    for (i = 0U; i < 8U; ++i) {
        store64_le(buffer + i * 8U, S->h[i]);
    }
    memcpy(out, buffer, outlen);
    secure_bzero(buffer, sizeof(buffer));
    return 0;
}

static int blake2b_hash(uint8_t *out, size_t outlen, const void *in, size_t inlen) {
    struct blake2b_state S;
    if (blake2b_init(&S, outlen) != 0) {
        return -1;
    }
    blake2b_update(&S, in, inlen);
    return blake2b_final(&S, out, outlen);
}

static int blake2b_long(uint8_t *out, size_t outlen, const uint8_t *in, size_t inlen) {
    uint8_t out_buffer[64];
    uint8_t outlen_bytes[4];
    size_t produced = 0U;
    struct blake2b_state S;
    if (out == NULL || outlen == 0U) {
        return -1;
    }
    store32_le(outlen_bytes, (uint32_t)outlen);
    if (outlen <= 64U) {
        if (blake2b_init(&S, outlen) != 0) {
            return -1;
        }
        blake2b_update(&S, outlen_bytes, sizeof(outlen_bytes));
        blake2b_update(&S, in, inlen);
        return blake2b_final(&S, out, outlen);
    }
    if (blake2b_init(&S, 64U) != 0) {
        return -1;
    }
    blake2b_update(&S, outlen_bytes, sizeof(outlen_bytes));
    blake2b_update(&S, in, inlen);
    if (blake2b_final(&S, out_buffer, 64U) != 0) {
        return -1;
    }
    memcpy(out, out_buffer, 32U);
    produced = 32U;
    while (outlen - produced > 64U) {
        if (blake2b_hash(out_buffer, 64U, out_buffer, 64U) != 0) {
            secure_bzero(out_buffer, sizeof(out_buffer));
            return -1;
        }
        memcpy(out + produced, out_buffer, 32U);
        produced += 32U;
    }
    if (blake2b_hash(out_buffer, outlen - produced, out_buffer, 64U) != 0) {
        secure_bzero(out_buffer, sizeof(out_buffer));
        return -1;
    }
    memcpy(out + produced, out_buffer, outlen - produced);
    secure_bzero(out_buffer, sizeof(out_buffer));
    return 0;
}

static inline uint64_t fBlaMka(uint64_t x, uint64_t y) {
    return x + y + 2U * (uint64_t)(uint32_t)x * (uint64_t)(uint32_t)y;
}

#define ARGON2_G(a, b, c, d) \
    do { \
        a = fBlaMka(a, b); \
        d = rotr64(d ^ a, 32); \
        c = fBlaMka(c, d); \
        b = rotr64(b ^ c, 24); \
        a = fBlaMka(a, b); \
        d = rotr64(d ^ a, 16); \
        c = fBlaMka(c, d); \
        b = rotr64(b ^ c, 63); \
    } while (0)

static void argon2_round(uint64_t *v) {
    ARGON2_G(v[0], v[4], v[8], v[12]);
    ARGON2_G(v[1], v[5], v[9], v[13]);
    ARGON2_G(v[2], v[6], v[10], v[14]);
    ARGON2_G(v[3], v[7], v[11], v[15]);
    ARGON2_G(v[0], v[5], v[10], v[15]);
    ARGON2_G(v[1], v[6], v[11], v[12]);
    ARGON2_G(v[2], v[7], v[8], v[13]);
    ARGON2_G(v[3], v[4], v[9], v[14]);
}

static void argon2_fill_block(const argon2_block *prev, const argon2_block *ref, argon2_block *next, int with_xor) {
    argon2_block block_r;
    argon2_block block_z;
    size_t i;
    for (i = 0U; i < ARGON2_QWORDS_IN_BLOCK; ++i) {
        block_r.v[i] = prev->v[i] ^ ref->v[i];
        block_z.v[i] = with_xor ? (block_r.v[i] ^ next->v[i]) : block_r.v[i];
    }
    for (i = 0U; i < 8U; ++i) {
        argon2_round(&block_z.v[i * 16U]);
    }
    for (i = 0U; i < 8U; ++i) {
        uint64_t tmp[16];
        size_t j;
        for (j = 0U; j < 16U; ++j) {
            size_t idx = (j % 2U) + (i * 2U) + (j / 2U) * 16U;
            tmp[j] = block_z.v[idx];
        }
        argon2_round(tmp);
        for (j = 0U; j < 16U; ++j) {
            size_t idx = (j % 2U) + (i * 2U) + (j / 2U) * 16U;
            block_z.v[idx] = tmp[j];
        }
    }
    for (i = 0U; i < ARGON2_QWORDS_IN_BLOCK; ++i) {
        next->v[i] = block_z.v[i] ^ block_r.v[i];
    }
}

static void argon2_generate_addresses(argon2_block *address_block, argon2_block *input_block, const argon2_block *zero_block) {
    argon2_block tmp = *zero_block;
    input_block->v[6]++;
    argon2_fill_block(zero_block, input_block, &tmp, 0);
    argon2_fill_block(zero_block, &tmp, address_block, 0);
}

static uint32_t argon2_index_alpha(uint32_t pass, uint32_t slice, uint32_t index, uint32_t lane_length, uint32_t segment_length, uint64_t pseudo_rand) {
    uint32_t reference_area_size;
    uint64_t relative_position;
    uint32_t start_position;
    if (pass == 0U) {
        if (slice == 0U) {
            reference_area_size = index - 1U;
        } else {
            reference_area_size = slice * segment_length + index - 1U;
        }
    } else {
        reference_area_size = lane_length - segment_length + index - 1U;
    }
    if (reference_area_size == 0U) {
        return 0U;
    }
    relative_position = pseudo_rand & UINT64_C(0xffffffff);
    relative_position = (relative_position * relative_position) >> 32;
    relative_position = (uint64_t)reference_area_size - 1U - (((uint64_t)reference_area_size * relative_position) >> 32);
    start_position = 0U;
    if (pass != 0U) {
        start_position = ((slice + 1U) * segment_length) % lane_length;
    }
    return (start_position + (uint32_t)relative_position) % lane_length;
}

static void argon2_initialize_memory(argon2_block *memory, const uint8_t *h0, uint32_t parallelism) {
    uint8_t blockhash[72];
    uint32_t lane;
    for (lane = 0U; lane < parallelism; ++lane) {
        memcpy(blockhash, h0, 64U);
        store32_le(blockhash + 64U, 0U);
        store32_le(blockhash + 68U, lane);
        (void)blake2b_long((uint8_t *)&memory[lane * 2U], ARGON2_BLOCK_SIZE, blockhash, sizeof(blockhash));
        store32_le(blockhash + 64U, 1U);
        (void)blake2b_long((uint8_t *)&memory[lane * 2U + 1U], ARGON2_BLOCK_SIZE, blockhash, sizeof(blockhash));
    }
    secure_bzero(blockhash, sizeof(blockhash));
}

static int argon2id_hash_raw(uint32_t t_cost, uint32_t m_cost, uint32_t parallelism,
                             const void *pwd, size_t pwdlen,
                             const void *salt, size_t saltlen,
                             void *hash, size_t hashlen) {
    uint32_t memory_blocks;
    uint32_t lane_length;
    uint32_t segment_length;
    uint8_t h0[64];
    uint8_t buf[1024];
    size_t off = 0U;
    struct blake2b_state S;
    argon2_block *memory = NULL;
    uint32_t pass;
    if (parallelism != 1U || hash == NULL || hashlen == 0U || salt == NULL || saltlen == 0U || pwd == NULL) {
        return -1;
    }
    if (m_cost < 8U * parallelism) {
        m_cost = 8U * parallelism;
    }
    memory_blocks = (m_cost / (ARGON2_SYNC_POINTS * parallelism)) * (ARGON2_SYNC_POINTS * parallelism);
    if (memory_blocks < 8U * parallelism) {
        memory_blocks = 8U * parallelism;
    }
    lane_length = memory_blocks / parallelism;
    segment_length = lane_length / ARGON2_SYNC_POINTS;
    if (blake2b_init(&S, sizeof(h0)) != 0) {
        return -1;
    }
    store32_le(buf + off, parallelism); off += 4U;
    store32_le(buf + off, (uint32_t)hashlen); off += 4U;
    store32_le(buf + off, m_cost); off += 4U;
    store32_le(buf + off, t_cost); off += 4U;
    store32_le(buf + off, ARGON2_VERSION); off += 4U;
    store32_le(buf + off, ARGON2_TYPE_ID); off += 4U;
    store32_le(buf + off, (uint32_t)pwdlen); off += 4U;
    memcpy(buf + off, pwd, pwdlen); off += pwdlen;
    store32_le(buf + off, (uint32_t)saltlen); off += 4U;
    memcpy(buf + off, salt, saltlen); off += saltlen;
    store32_le(buf + off, 0U); off += 4U;
    store32_le(buf + off, 0U); off += 4U;
    store32_le(buf + off, 0U); off += 4U;
    store32_le(buf + off, 0U); off += 4U;
    blake2b_update(&S, buf, off);
    if (blake2b_final(&S, h0, sizeof(h0)) != 0) {
        secure_bzero(buf, sizeof(buf));
        return -1;
    }
    memory = (argon2_block *)calloc(memory_blocks, sizeof(argon2_block));
    if (memory == NULL) {
        secure_bzero(h0, sizeof(h0));
        secure_bzero(buf, sizeof(buf));
        return -1;
    }
    argon2_initialize_memory(memory, h0, parallelism);
    for (pass = 0U; pass < t_cost; ++pass) {
        uint32_t slice;
        for (slice = 0U; slice < ARGON2_SYNC_POINTS; ++slice) {
            uint32_t index;
            uint32_t start_index = (pass == 0U && slice == 0U) ? 2U : 0U;
            int data_independent = (pass == 0U && slice < 2U) ? 1 : 0;
            argon2_block zero_block;
            argon2_block input_block;
            argon2_block address_block;
            memset(&zero_block, 0, sizeof(zero_block));
            memset(&input_block, 0, sizeof(input_block));
            memset(&address_block, 0, sizeof(address_block));
            if (data_independent != 0) {
                input_block.v[0] = pass;
                input_block.v[1] = 0U;
                input_block.v[2] = slice;
                input_block.v[3] = memory_blocks;
                input_block.v[4] = t_cost;
                input_block.v[5] = ARGON2_TYPE_ID;
            }
            for (index = start_index; index < segment_length; ++index) {
                uint32_t curr = slice * segment_length + index;
                uint32_t prev = (curr == 0U) ? (lane_length - 1U) : (curr - 1U);
                uint64_t pseudo_rand;
                uint32_t ref_index;
                if (data_independent != 0) {
                    if ((index - start_index) % ARGON2_QWORDS_IN_BLOCK == 0U) {
                        argon2_generate_addresses(&address_block, &input_block, &zero_block);
                    }
                    pseudo_rand = address_block.v[(index - start_index) % ARGON2_QWORDS_IN_BLOCK];
                } else {
                    pseudo_rand = memory[prev].v[0];
                }
                ref_index = argon2_index_alpha(pass, slice, index, lane_length, segment_length, pseudo_rand);
                argon2_fill_block(&memory[prev], &memory[ref_index], &memory[curr], pass != 0U);
            }
        }
    }
    if (blake2b_long((uint8_t *)hash, hashlen, (const uint8_t *)&memory[lane_length - 1U], ARGON2_BLOCK_SIZE) != 0) {
        free(memory);
        secure_bzero(h0, sizeof(h0));
        secure_bzero(buf, sizeof(buf));
        return -1;
    }
    secure_bzero(memory, (size_t)memory_blocks * sizeof(argon2_block));
    free(memory);
    secure_bzero(h0, sizeof(h0));
    secure_bzero(buf, sizeof(buf));
    return 0;
}

static int argon2id_hash_encoded(uint32_t t_cost, uint32_t m_cost, uint32_t parallelism,
                                 const void *pwd, size_t pwdlen,
                                 const void *salt, size_t saltlen,
                                 size_t hashlen,
                                 char *encoded, size_t encodedlen) {
    uint8_t hash[64];
    char salt_b64[64];
    char hash_b64[128];
    int salt_chars;
    int hash_chars;
    int wrote;
    if (hashlen > sizeof(hash)) {
        return -1;
    }
    if (argon2id_hash_raw(t_cost, m_cost, parallelism, pwd, pwdlen, salt, saltlen, hash, hashlen) != 0) {
        return -1;
    }
    salt_chars = base64_encode(salt_b64, sizeof(salt_b64), (const uint8_t *)salt, saltlen);
    hash_chars = base64_encode(hash_b64, sizeof(hash_b64), hash, hashlen);
    secure_bzero(hash, sizeof(hash));
    if (salt_chars < 0 || hash_chars < 0) {
        return -1;
    }
    wrote = snprintf(encoded, encodedlen, "$argon2id$v=%u$m=%u,t=%u,p=%u$%s$%s",
                     ARGON2_VERSION, m_cost, t_cost, parallelism, salt_b64, hash_b64);
    if (wrote < 0 || (size_t)wrote >= encodedlen) {
        return -1;
    }
    return 0;
}

static int constant_time_eq(const uint8_t *a, const uint8_t *b, size_t len) {
    size_t i;
    uint8_t diff = 0U;
    for (i = 0U; i < len; ++i) {
        diff |= a[i] ^ b[i];
    }
    return diff == 0U;
}

static int argon2id_verify(const char *encoded, const void *pwd, size_t pwdlen) {
    char copy[MAX_HASH];
    char *p;
    char *parts[6];
    int idx = 0;
    uint32_t version = 0U;
    uint32_t m_cost = 0U;
    uint32_t t_cost = 0U;
    uint32_t parallelism = 0U;
    uint8_t salt[64];
    uint8_t expected[64];
    uint8_t actual[64];
    int salt_len;
    int hash_len;
    if (encoded == NULL || pwd == NULL) {
        return -1;
    }
    if (string_copy(copy, sizeof(copy), encoded) != 0) {
        return -1;
    }
    p = copy;
    while (*p != '\0' && idx < 6) {
        char *next;
        if (*p == '$') {
            p++;
        }
        next = strchr(p, '$');
        if (next != NULL) {
            *next = '\0';
        }
        parts[idx++] = p;
        if (next == NULL) {
            break;
        }
        p = next + 1;
    }
    if (idx != 5 || strcmp(parts[0], "argon2id") != 0) {
        return -1;
    }
    if (sscanf(parts[1], "v=%u", &version) != 1 || version != ARGON2_VERSION) {
        return -1;
    }
    if (sscanf(parts[2], "m=%u,t=%u,p=%u", &m_cost, &t_cost, &parallelism) != 3) {
        return -1;
    }
    salt_len = base64_decode(salt, sizeof(salt), parts[3], strlen(parts[3]));
    hash_len = base64_decode(expected, sizeof(expected), parts[4], strlen(parts[4]));
    if (salt_len <= 0 || hash_len <= 0) {
        return -1;
    }
    if (argon2id_hash_raw(t_cost, m_cost, parallelism, pwd, pwdlen, salt, (size_t)salt_len, actual, (size_t)hash_len) != 0) {
        secure_bzero(salt, sizeof(salt));
        secure_bzero(expected, sizeof(expected));
        return -1;
    }
    secure_bzero(salt, sizeof(salt));
    if (constant_time_eq(expected, actual, (size_t)hash_len)) {
        secure_bzero(expected, sizeof(expected));
        secure_bzero(actual, sizeof(actual));
        return 0;
    }
    secure_bzero(expected, sizeof(expected));
    secure_bzero(actual, sizeof(actual));
    return -1;
}

static int create_password_hash(const char *password, char *encoded, size_t encoded_len) {
    uint8_t salt[ARGON2_SALT_LEN];
    int fd;
    ssize_t rd;
    fd = open("/dev/urandom", O_RDONLY);
    if (fd < 0) {
        return -1;
    }
    rd = read(fd, salt, sizeof(salt));
    close(fd);
    if (rd != (ssize_t)sizeof(salt)) {
        secure_bzero(salt, sizeof(salt));
        return -1;
    }
    if (argon2id_hash_encoded(ARGON2_T_COST, ARGON2_M_COST, ARGON2_PARALLELISM,
                              password, strlen(password), salt, sizeof(salt),
                              ARGON2_HASH_LEN, encoded, encoded_len) != 0) {
        secure_bzero(salt, sizeof(salt));
        return -1;
    }
    secure_bzero(salt, sizeof(salt));
    return 0;
}

static int parse_options(int argc, char **argv, struct options *opts) {
    int i;
    memset(opts, 0, sizeof(*opts));
    if (argc < 2) {
        return 0;
    }
    opts->command = argv[1];
    for (i = 2; i < argc; ++i) {
        if (strcmp(argv[i], "--user") == 0 && i + 1 < argc) {
            if (string_copy(opts->user, sizeof(opts->user), argv[++i]) != 0) {
                return -1;
            }
        } else if (strcmp(argv[i], "--fullname") == 0 && i + 1 < argc) {
            if (string_copy(opts->fullname, sizeof(opts->fullname), argv[++i]) != 0) {
                return -1;
            }
        } else if (strcmp(argv[i], "--uid") == 0 && i + 1 < argc) {
            opts->uid = (uint32_t)strtoul(argv[++i], NULL, 10);
            opts->uid_set = 1;
        } else if (strcmp(argv[i], "--birth") == 0 && i + 1 < argc) {
            if (string_copy(opts->birthdate, sizeof(opts->birthdate), argv[++i]) != 0) {
                return -1;
            }
        } else if (strcmp(argv[i], "--groups") == 0 && i + 1 < argc) {
            if (string_copy(opts->groups, sizeof(opts->groups), argv[++i]) != 0) {
                return -1;
            }
        } else if (strcmp(argv[i], "--add-groups") == 0 && i + 1 < argc) {
            if (string_copy(opts->add_groups, sizeof(opts->add_groups), argv[++i]) != 0) {
                return -1;
            }
        } else if (strcmp(argv[i], "--del-groups") == 0 && i + 1 < argc) {
            if (string_copy(opts->del_groups, sizeof(opts->del_groups), argv[++i]) != 0) {
                return -1;
            }
        } else if (strcmp(argv[i], "--home") == 0 && i + 1 < argc) {
            if (string_copy(opts->home, sizeof(opts->home), argv[++i]) != 0) {
                return -1;
            }
        } else if (strcmp(argv[i], "--shell") == 0 && i + 1 < argc) {
            if (string_copy(opts->shell, sizeof(opts->shell), argv[++i]) != 0) {
                return -1;
            }
        } else if (strcmp(argv[i], "--password") == 0 && i + 1 < argc) {
            if (string_copy(opts->password, sizeof(opts->password), argv[++i]) != 0) {
                return -1;
            }
        } else if (strcmp(argv[i], "--remove-home") == 0) {
            opts->remove_home = 1;
        } else {
            return -1;
        }
    }
    return 0;
}

static int validate_group_csv_exists(const char *csv, const struct group_entry *groups, size_t group_count) {
    char copy[MAX_GROUP_CSV];
    char *saveptr = NULL;
    char *tok;
    if (csv == NULL || csv[0] == '\0') {
        return 0;
    }
    if (string_copy(copy, sizeof(copy), csv) != 0) {
        return -1;
    }
    tok = strtok_r(copy, ",", &saveptr);
    while (tok != NULL) {
        trim_whitespace(tok);
        if (tok[0] != '\0' && find_group_by_name(groups, group_count, tok) < 0) {
            print_error("El grupo '%s' no existe.", tok);
            return -1;
        }
        tok = strtok_r(NULL, ",", &saveptr);
    }
    return 0;
}

static void print_user_summary(const struct user_entry *user, const struct group_entry *groups, size_t group_count) {
    char copy[MAX_GROUP_CSV];
    char *saveptr = NULL;
    char *tok;
    printf("\n+------------------------------------------------------+\n");
    printf("| Resumen del usuario creado                           |\n");
    printf("+------------------------------------------------------+\n");
    printf(" Usuario      : %s\n", user->username);
    printf(" UID/GID      : %" PRIu32 "/%" PRIu32 "\n", user->uid, user->primary_gid);
    printf(" Nombre       : %s\n", user->fullname);
    printf(" Nacimiento   : %s\n", user->birthdate);
    printf(" Home         : %s\n", user->homedir);
    printf(" Shell        : %s\n", user->shell);
    printf(" Recursos     : %s\n", user->resource_groups[0] ? user->resource_groups : "(sin grupos)");
    printf(" Permisos:\n");
    if (string_copy(copy, sizeof(copy), user->resource_groups) != 0) {
        return;
    }
    tok = strtok_r(copy, ",", &saveptr);
    while (tok != NULL) {
        ssize_t idx = find_group_by_name(groups, group_count, tok);
        if (idx >= 0) {
            printf("   - %-12s %s\n", groups[(size_t)idx].name, groups[(size_t)idx].description);
        }
        tok = strtok_r(NULL, ",", &saveptr);
    }
}

static int command_init(void) {
    struct db_locks locks;
    struct user_entry users[MAX_USERS];
    struct shadow_entry shadows[MAX_SHADOWS];
    struct group_entry groups[MAX_GROUPS];
    size_t user_count = 0U;
    size_t shadow_count = 0U;
    size_t group_count = 0U;
    size_t i;
    if (ensure_root("init") != 0) {
        return 1;
    }
    if (lock_db_files(&locks) != 0) {
        print_error("No se pudieron bloquear los archivos de base de datos.");
        return 1;
    }
    memset(users, 0, sizeof(users));
    memset(shadows, 0, sizeof(shadows));
    memset(groups, 0, sizeof(groups));
    if (load_users(users, &user_count) != 0 || load_shadows(shadows, &shadow_count) != 0 || load_groups(groups, &group_count) != 0) {
        unlock_db_files(&locks);
        print_error("No se pudieron cargar las bases de datos existentes.");
        return 1;
    }
    if (group_count == 0U) {
        for (i = 0U; i < sizeof(g_default_groups) / sizeof(g_default_groups[0]); ++i) {
            (void)string_copy(groups[group_count].name, sizeof(groups[group_count].name), g_default_groups[i].name);
            groups[group_count].gid = g_default_groups[i].gid;
            (void)string_copy(groups[group_count].description, sizeof(groups[group_count].description), g_default_groups[i].description);
            (void)string_copy(groups[group_count].members, sizeof(groups[group_count].members), g_default_groups[i].members);
            group_count++;
        }
    }
    if (find_user_by_name(users, user_count, "root") < 0 && user_count < MAX_USERS) {
        (void)string_copy(users[user_count].username, sizeof(users[user_count].username), "root");
        users[user_count].uid = 0U;
        users[user_count].primary_gid = 0U;
        (void)string_copy(users[user_count].fullname, sizeof(users[user_count].fullname), "Administrador");
        (void)string_copy(users[user_count].birthdate, sizeof(users[user_count].birthdate), "0000-00-00");
        (void)string_copy(users[user_count].homedir, sizeof(users[user_count].homedir), "/root");
        (void)string_copy(users[user_count].shell, sizeof(users[user_count].shell), "/bin/sh");
        (void)string_copy(users[user_count].resource_groups, sizeof(users[user_count].resource_groups), "wheel");
        user_count++;
    }
    if (find_shadow_by_name(shadows, shadow_count, "root") < 0 && shadow_count < MAX_SHADOWS) {
        (void)string_copy(shadows[shadow_count].username, sizeof(shadows[shadow_count].username), "root");
        (void)string_copy(shadows[shadow_count].hash, sizeof(shadows[shadow_count].hash), "!");
        shadows[shadow_count].last_change_days = days_since_epoch();
        shadows[shadow_count].min_days = 0;
        shadows[shadow_count].max_days = 99999;
        shadows[shadow_count].warn_days = 7;
        shadow_count++;
    }
    apply_memberships_from_users(groups, group_count, users, user_count);
    if (save_users(users, user_count) != 0 || save_shadows(shadows, shadow_count) != 0 || save_groups(groups, group_count) != 0) {
        unlock_db_files(&locks);
        print_error("No se pudieron escribir las bases de datos.");
        return 1;
    }
    unlock_db_files(&locks);
    print_success("Base de datos inicializada correctamente.");
    return command_grouplist();
}

static int command_useradd(struct options *opts) {
    struct db_locks locks;
    struct user_entry users[MAX_USERS];
    struct shadow_entry shadows[MAX_SHADOWS];
    struct group_entry groups[MAX_GROUPS];
    struct user_entry new_user;
    struct shadow_entry new_shadow;
    size_t user_count = 0U;
    size_t shadow_count = 0U;
    size_t group_count = 0U;
    char groups_csv[MAX_GROUP_CSV] = "";
    char password[256];
    char hash[MAX_HASH];
    if (ensure_root("useradd") != 0) {
        return 1;
    }
    if (lock_db_files(&locks) != 0) {
        print_error("No se pudieron bloquear los archivos.");
        return 1;
    }
    if (load_users(users, &user_count) != 0 || load_shadows(shadows, &shadow_count) != 0 || load_groups(groups, &group_count) != 0) {
        unlock_db_files(&locks);
        print_error("No se pudieron cargar las bases de datos.");
        return 1;
    }
    memset(&new_user, 0, sizeof(new_user));
    memset(&new_shadow, 0, sizeof(new_shadow));
    memset(password, 0, sizeof(password));
    memset(hash, 0, sizeof(hash));

    if (opts->user[0] == '\0' && prompt_input("Usuario", opts->user, sizeof(opts->user), NULL) != 0) {
        unlock_db_files(&locks);
        return 1;
    }
    if (!validate_username(opts->user)) {
        unlock_db_files(&locks);
        print_error("Nombre de usuario inválido.");
        return 1;
    }
    if (find_user_by_name(users, user_count, opts->user) >= 0) {
        unlock_db_files(&locks);
        print_error("El usuario '%s' ya existe.", opts->user);
        return 1;
    }
    if (opts->fullname[0] == '\0' && prompt_input("Nombre completo", opts->fullname, sizeof(opts->fullname), NULL) != 0) {
        unlock_db_files(&locks);
        return 1;
    }
    if (!validate_fullname(opts->fullname)) {
        unlock_db_files(&locks);
        print_error("Nombre completo inválido.");
        return 1;
    }
    if (!opts->uid_set) {
        char uidbuf[32] = "";
        char defuid[32];
        snprintf(defuid, sizeof(defuid), "%" PRIu32, next_available_uid(users, user_count));
        if (prompt_input("UID", uidbuf, sizeof(uidbuf), defuid) != 0) {
            unlock_db_files(&locks);
            return 1;
        }
        opts->uid = (uint32_t)strtoul(uidbuf, NULL, 10);
        opts->uid_set = 1;
    }
    if (find_user_by_uid(users, user_count, opts->uid) >= 0) {
        unlock_db_files(&locks);
        print_error("El UID %" PRIu32 " ya está en uso.", opts->uid);
        return 1;
    }
    if (opts->birthdate[0] == '\0' && prompt_input("Fecha de nacimiento (YYYY-MM-DD)", opts->birthdate, sizeof(opts->birthdate), NULL) != 0) {
        unlock_db_files(&locks);
        return 1;
    }
    if (!validate_birthdate(opts->birthdate)) {
        unlock_db_files(&locks);
        print_error("Fecha de nacimiento inválida.");
        return 1;
    }
    if (opts->home[0] == '\0') {
        char defhome[MAX_HOME];
        snprintf(defhome, sizeof(defhome), "/home/%s", opts->user);
        if (prompt_input("Directorio HOME", opts->home, sizeof(opts->home), defhome) != 0) {
            unlock_db_files(&locks);
            return 1;
        }
    }
    if (opts->shell[0] == '\0' && prompt_input("Shell", opts->shell, sizeof(opts->shell), "/bin/sh") != 0) {
        unlock_db_files(&locks);
        return 1;
    }
    if (opts->groups[0] == '\0' && prompt_input("Grupos de recursos (coma)", opts->groups, sizeof(opts->groups), "users") != 0) {
        unlock_db_files(&locks);
        return 1;
    }
    if (csv_normalize(groups_csv, sizeof(groups_csv), opts->groups) != 0) {
        unlock_db_files(&locks);
        print_error("Lista de grupos inválida.");
        return 1;
    }
    if (csv_add(groups_csv, sizeof(groups_csv), "users") != 0) {
        unlock_db_files(&locks);
        print_error("No se pudo añadir el grupo users.");
        return 1;
    }
    if (validate_group_csv_exists(groups_csv, groups, group_count) != 0) {
        unlock_db_files(&locks);
        return 1;
    }
    if (opts->password[0] != '\0') {
        if (string_copy(password, sizeof(password), opts->password) != 0) {
            unlock_db_files(&locks);
            return 1;
        }
    } else if (prompt_password_twice(password, sizeof(password)) != 0) {
        unlock_db_files(&locks);
        return 1;
    }
    if (create_password_hash(password, hash, sizeof(hash)) != 0) {
        secure_bzero(password, sizeof(password));
        unlock_db_files(&locks);
        print_error("No se pudo generar el hash de contraseña.");
        return 1;
    }
    (void)string_copy(new_user.username, sizeof(new_user.username), opts->user);
    new_user.uid = opts->uid;
    new_user.primary_gid = opts->uid;
    (void)string_copy(new_user.fullname, sizeof(new_user.fullname), opts->fullname);
    (void)string_copy(new_user.birthdate, sizeof(new_user.birthdate), opts->birthdate);
    (void)string_copy(new_user.homedir, sizeof(new_user.homedir), opts->home);
    (void)string_copy(new_user.shell, sizeof(new_user.shell), opts->shell);
    (void)string_copy(new_user.resource_groups, sizeof(new_user.resource_groups), groups_csv);
    users[user_count++] = new_user;

    (void)string_copy(new_shadow.username, sizeof(new_shadow.username), opts->user);
    (void)string_copy(new_shadow.hash, sizeof(new_shadow.hash), hash);
    new_shadow.last_change_days = days_since_epoch();
    new_shadow.min_days = 0;
    new_shadow.max_days = 99999;
    new_shadow.warn_days = 7;
    shadows[shadow_count++] = new_shadow;

    apply_memberships_from_users(groups, group_count, users, user_count);
    if (save_users(users, user_count) != 0 || save_shadows(shadows, shadow_count) != 0 || save_groups(groups, group_count) != 0) {
        secure_bzero(password, sizeof(password));
        secure_bzero(hash, sizeof(hash));
        unlock_db_files(&locks);
        print_error("No se pudieron guardar los cambios.");
        return 1;
    }
    if (create_home_directory(new_user.homedir, new_user.uid, new_user.primary_gid) != 0) {
        print_info("Aviso: no se pudo crear o ajustar %s (%s)", new_user.homedir, strerror(errno));
    }
    unlock_db_files(&locks);
    secure_bzero(password, sizeof(password));
    secure_bzero(hash, sizeof(hash));
    print_success("Usuario '%s' creado correctamente.", new_user.username);
    print_user_summary(&new_user, groups, group_count);
    return 0;
}

static int command_userdel(struct options *opts) {
    struct db_locks locks;
    struct user_entry users[MAX_USERS];
    struct shadow_entry shadows[MAX_SHADOWS];
    struct group_entry groups[MAX_GROUPS];
    size_t user_count = 0U;
    size_t shadow_count = 0U;
    size_t group_count = 0U;
    ssize_t uidx;
    ssize_t sidx;
    char home_copy[MAX_HOME];
    if (ensure_root("userdel") != 0) {
        return 1;
    }
    if (opts->user[0] == '\0' && prompt_input("Usuario a eliminar", opts->user, sizeof(opts->user), NULL) != 0) {
        return 1;
    }
    if (!validate_username(opts->user)) {
        print_error("Usuario inválido.");
        return 1;
    }
    if (lock_db_files(&locks) != 0) {
        print_error("No se pudieron bloquear los archivos.");
        return 1;
    }
    if (load_users(users, &user_count) != 0 || load_shadows(shadows, &shadow_count) != 0 || load_groups(groups, &group_count) != 0) {
        unlock_db_files(&locks);
        print_error("No se pudieron cargar las bases de datos.");
        return 1;
    }
    uidx = find_user_by_name(users, user_count, opts->user);
    if (uidx < 0) {
        unlock_db_files(&locks);
        print_error("El usuario '%s' no existe.", opts->user);
        return 1;
    }
    if (strcmp(users[(size_t)uidx].username, "root") == 0) {
        unlock_db_files(&locks);
        print_error("No se puede eliminar root.");
        return 1;
    }
    (void)string_copy(home_copy, sizeof(home_copy), users[(size_t)uidx].homedir);
    memmove(&users[(size_t)uidx], &users[(size_t)uidx + 1U], (user_count - (size_t)uidx - 1U) * sizeof(users[0]));
    user_count--;
    sidx = find_shadow_by_name(shadows, shadow_count, opts->user);
    if (sidx >= 0) {
        memmove(&shadows[(size_t)sidx], &shadows[(size_t)sidx + 1U], (shadow_count - (size_t)sidx - 1U) * sizeof(shadows[0]));
        shadow_count--;
    }
    apply_memberships_from_users(groups, group_count, users, user_count);
    if (save_users(users, user_count) != 0 || save_shadows(shadows, shadow_count) != 0 || save_groups(groups, group_count) != 0) {
        unlock_db_files(&locks);
        print_error("No se pudieron guardar los cambios.");
        return 1;
    }
    unlock_db_files(&locks);
    if (opts->remove_home) {
        if (recursive_delete_path(home_copy) != 0) {
            print_info("Aviso: no se pudo eliminar %s (%s)", home_copy, strerror(errno));
        }
    }
    print_success("Usuario '%s' eliminado correctamente.", opts->user);
    return 0;
}

static int command_usermod(struct options *opts) {
    struct db_locks locks;
    struct user_entry users[MAX_USERS];
    struct shadow_entry shadows[MAX_SHADOWS];
    struct group_entry groups[MAX_GROUPS];
    size_t user_count = 0U;
    size_t shadow_count = 0U;
    size_t group_count = 0U;
    ssize_t uidx;
    char final_groups[MAX_GROUP_CSV];
    if (ensure_root("usermod") != 0) {
        return 1;
    }
    if (opts->user[0] == '\0' && prompt_input("Usuario a modificar", opts->user, sizeof(opts->user), NULL) != 0) {
        return 1;
    }
    if (lock_db_files(&locks) != 0) {
        print_error("No se pudieron bloquear los archivos.");
        return 1;
    }
    if (load_users(users, &user_count) != 0 || load_shadows(shadows, &shadow_count) != 0 || load_groups(groups, &group_count) != 0) {
        unlock_db_files(&locks);
        print_error("No se pudieron cargar las bases de datos.");
        return 1;
    }
    uidx = find_user_by_name(users, user_count, opts->user);
    if (uidx < 0) {
        unlock_db_files(&locks);
        print_error("El usuario '%s' no existe.", opts->user);
        return 1;
    }
    (void)string_copy(final_groups, sizeof(final_groups), users[(size_t)uidx].resource_groups);
    if (opts->fullname[0] != '\0') {
        if (!validate_fullname(opts->fullname)) {
            unlock_db_files(&locks);
            print_error("Nombre completo inválido.");
            return 1;
        }
        (void)string_copy(users[(size_t)uidx].fullname, sizeof(users[(size_t)uidx].fullname), opts->fullname);
    }
    if (opts->birthdate[0] != '\0') {
        if (!validate_birthdate(opts->birthdate)) {
            unlock_db_files(&locks);
            print_error("Fecha de nacimiento inválida.");
            return 1;
        }
        (void)string_copy(users[(size_t)uidx].birthdate, sizeof(users[(size_t)uidx].birthdate), opts->birthdate);
    }
    if (opts->home[0] != '\0') {
        (void)string_copy(users[(size_t)uidx].homedir, sizeof(users[(size_t)uidx].homedir), opts->home);
    }
    if (opts->shell[0] != '\0') {
        (void)string_copy(users[(size_t)uidx].shell, sizeof(users[(size_t)uidx].shell), opts->shell);
    }
    if (opts->uid_set) {
        ssize_t other = find_user_by_uid(users, user_count, opts->uid);
        if (other >= 0 && other != uidx) {
            unlock_db_files(&locks);
            print_error("El UID %" PRIu32 " ya está en uso.", opts->uid);
            return 1;
        }
        users[(size_t)uidx].uid = opts->uid;
        users[(size_t)uidx].primary_gid = opts->uid;
    }
    if (opts->groups[0] != '\0') {
        if (csv_normalize(final_groups, sizeof(final_groups), opts->groups) != 0) {
            unlock_db_files(&locks);
            print_error("Lista de grupos inválida.");
            return 1;
        }
    }
    if (opts->add_groups[0] != '\0') {
        char copy[MAX_GROUP_CSV];
        char *saveptr = NULL;
        char *tok;
        if (string_copy(copy, sizeof(copy), opts->add_groups) != 0) {
            unlock_db_files(&locks);
            return 1;
        }
        tok = strtok_r(copy, ",", &saveptr);
        while (tok != NULL) {
            trim_whitespace(tok);
            if (tok[0] != '\0' && csv_add(final_groups, sizeof(final_groups), tok) != 0) {
                unlock_db_files(&locks);
                print_error("No se pudo añadir el grupo '%s'.", tok);
                return 1;
            }
            tok = strtok_r(NULL, ",", &saveptr);
        }
    }
    if (opts->del_groups[0] != '\0') {
        char copy[MAX_GROUP_CSV];
        char *saveptr = NULL;
        char *tok;
        if (string_copy(copy, sizeof(copy), opts->del_groups) != 0) {
            unlock_db_files(&locks);
            return 1;
        }
        tok = strtok_r(copy, ",", &saveptr);
        while (tok != NULL) {
            trim_whitespace(tok);
            if (tok[0] != '\0') {
                csv_remove(final_groups, tok);
            }
            tok = strtok_r(NULL, ",", &saveptr);
        }
    }
    if (csv_add(final_groups, sizeof(final_groups), "users") != 0) {
        unlock_db_files(&locks);
        print_error("No se pudo mantener el grupo users.");
        return 1;
    }
    if (validate_group_csv_exists(final_groups, groups, group_count) != 0) {
        unlock_db_files(&locks);
        return 1;
    }
    (void)string_copy(users[(size_t)uidx].resource_groups, sizeof(users[(size_t)uidx].resource_groups), final_groups);
    if (opts->password[0] != '\0') {
        ssize_t sidx = find_shadow_by_name(shadows, shadow_count, opts->user);
        char hash[MAX_HASH];
        if (sidx < 0 || create_password_hash(opts->password, hash, sizeof(hash)) != 0) {
            unlock_db_files(&locks);
            print_error("No se pudo actualizar la contraseña.");
            return 1;
        }
        (void)string_copy(shadows[(size_t)sidx].hash, sizeof(shadows[(size_t)sidx].hash), hash);
        shadows[(size_t)sidx].last_change_days = days_since_epoch();
        secure_bzero(hash, sizeof(hash));
    }
    apply_memberships_from_users(groups, group_count, users, user_count);
    if (save_users(users, user_count) != 0 || save_shadows(shadows, shadow_count) != 0 || save_groups(groups, group_count) != 0) {
        unlock_db_files(&locks);
        print_error("No se pudieron guardar los cambios.");
        return 1;
    }
    unlock_db_files(&locks);
    print_success("Usuario '%s' modificado correctamente.", opts->user);
    return 0;
}

static int command_passwd(struct options *opts) {
    struct db_locks locks;
    struct user_entry users[MAX_USERS];
    struct shadow_entry shadows[MAX_SHADOWS];
    struct group_entry groups[MAX_GROUPS];
    size_t user_count = 0U;
    size_t shadow_count = 0U;
    size_t group_count = 0U;
    ssize_t uidx;
    ssize_t sidx;
    char current[256];
    char newpass[256];
    char hash[MAX_HASH];
    int is_root = geteuid() == 0;
    memset(current, 0, sizeof(current));
    memset(newpass, 0, sizeof(newpass));
    memset(hash, 0, sizeof(hash));
    if (lock_db_files(&locks) != 0) {
        print_error("No se pudieron bloquear los archivos.");
        return 1;
    }
    if (load_users(users, &user_count) != 0 || load_shadows(shadows, &shadow_count) != 0 || load_groups(groups, &group_count) != 0) {
        (void)groups;
        (void)group_count;
        unlock_db_files(&locks);
        print_error("No se pudieron cargar las bases de datos.");
        return 1;
    }
    if (is_root) {
        if (opts->user[0] == '\0' && prompt_input("Usuario", opts->user, sizeof(opts->user), NULL) != 0) {
            unlock_db_files(&locks);
            return 1;
        }
    } else {
        uid_t uid = getuid();
        uidx = find_user_by_uid(users, user_count, (uint32_t)uid);
        if (uidx < 0) {
            unlock_db_files(&locks);
            print_error("No se encontró tu cuenta en la base de datos de Eclipse.");
            return 1;
        }
        if (opts->user[0] != '\0' && strcmp(opts->user, users[(size_t)uidx].username) != 0) {
            unlock_db_files(&locks);
            print_error("Solo puedes cambiar tu propia contraseña.");
            return 1;
        }
        (void)string_copy(opts->user, sizeof(opts->user), users[(size_t)uidx].username);
    }
    uidx = find_user_by_name(users, user_count, opts->user);
    sidx = find_shadow_by_name(shadows, shadow_count, opts->user);
    if (uidx < 0 || sidx < 0) {
        unlock_db_files(&locks);
        print_error("El usuario '%s' no existe.", opts->user);
        return 1;
    }
    if (!is_root) {
        if (read_hidden_line("Contraseña actual: ", current, sizeof(current)) != 0) {
            unlock_db_files(&locks);
            print_error("No se pudo leer la contraseña actual.");
            return 1;
        }
        if (argon2id_verify(shadows[(size_t)sidx].hash, current, strlen(current)) != 0) {
            secure_bzero(current, sizeof(current));
            unlock_db_files(&locks);
            print_error("La contraseña actual es incorrecta.");
            return 1;
        }
    }
    if (prompt_password_twice(newpass, sizeof(newpass)) != 0) {
        secure_bzero(current, sizeof(current));
        unlock_db_files(&locks);
        return 1;
    }
    if (create_password_hash(newpass, hash, sizeof(hash)) != 0) {
        secure_bzero(current, sizeof(current));
        secure_bzero(newpass, sizeof(newpass));
        unlock_db_files(&locks);
        print_error("No se pudo generar el nuevo hash.");
        return 1;
    }
    (void)string_copy(shadows[(size_t)sidx].hash, sizeof(shadows[(size_t)sidx].hash), hash);
    shadows[(size_t)sidx].last_change_days = days_since_epoch();
    if (save_shadows(shadows, shadow_count) != 0) {
        secure_bzero(current, sizeof(current));
        secure_bzero(newpass, sizeof(newpass));
        secure_bzero(hash, sizeof(hash));
        unlock_db_files(&locks);
        print_error("No se pudo guardar la nueva contraseña.");
        return 1;
    }
    secure_bzero(current, sizeof(current));
    secure_bzero(newpass, sizeof(newpass));
    secure_bzero(hash, sizeof(hash));
    unlock_db_files(&locks);
    print_success("Contraseña actualizada correctamente para '%s'.", opts->user);
    return 0;
}

static int command_userlist(void) {
    struct user_entry users[MAX_USERS];
    size_t user_count = 0U;
    size_t i;
    print_header();
    if (load_users(users, &user_count) != 0) {
        print_error("No se pudo leer %s", USERS_FILE);
        return 1;
    }
    printf("+------+----------------------------------+------------------------------+------------+-------------------------+------------------------------+\n");
    printf("| UID  | Usuario                          | Nombre completo              | Nacimiento | Home                    | Grupos                       |\n");
    printf("+------+----------------------------------+------------------------------+------------+-------------------------+------------------------------+\n");
    for (i = 0U; i < user_count; ++i) {
        printf("| %-4" PRIu32 " | %-32s | %-28s | %-10s | %-23s | %-28s |\n",
               users[i].uid, users[i].username, users[i].fullname,
               users[i].birthdate, users[i].homedir, users[i].resource_groups);
    }
    printf("+------+----------------------------------+------------------------------+------------+-------------------------+------------------------------+\n");
    return 0;
}

static int command_grouplist(void) {
    struct group_entry groups[MAX_GROUPS];
    size_t group_count = 0U;
    size_t i;
    print_header();
    if (load_groups(groups, &group_count) != 0) {
        print_error("No se pudo leer %s", GROUPS_FILE);
        return 1;
    }
    printf("+--------------+------+--------------------------------------------------------+------------------------------+\n");
    printf("| Grupo        | GID  | Descripción                                            | Miembros                     |\n");
    printf("+--------------+------+--------------------------------------------------------+------------------------------+\n");
    for (i = 0U; i < group_count; ++i) {
        printf("| %-12s | %-4" PRIu32 " | %-54s | %-28s |\n",
               groups[i].name, groups[i].gid, groups[i].description,
               groups[i].members[0] ? groups[i].members : "-");
    }
    printf("+--------------+------+--------------------------------------------------------+------------------------------+\n");
    return 0;
}

static int interactive_menu(const char *progname) {
    char choice[16];
    for (;;) {
        print_header();
        printf("1) init\n");
        printf("2) useradd\n");
        printf("3) userdel\n");
        printf("4) usermod\n");
        printf("5) passwd\n");
        printf("6) userlist\n");
        printf("7) grouplist\n");
        printf("8) help\n");
        printf("9) salir\n\n");
        if (prompt_input("Selecciona una opción", choice, sizeof(choice), NULL) != 0) {
            return 1;
        }
        switch (choice[0]) {
            case '1': return command_init();
            case '2': {
                struct options opts;
                memset(&opts, 0, sizeof(opts));
                return command_useradd(&opts);
            }
            case '3': {
                struct options opts;
                memset(&opts, 0, sizeof(opts));
                return command_userdel(&opts);
            }
            case '4': {
                struct options opts;
                memset(&opts, 0, sizeof(opts));
                return command_usermod(&opts);
            }
            case '5': {
                struct options opts;
                memset(&opts, 0, sizeof(opts));
                return command_passwd(&opts);
            }
            case '6': return command_userlist();
            case '7': return command_grouplist();
            case '8': print_usage(progname); return 0;
            case '9': return 0;
            default:
                print_error("Opción inválida.");
                clear_stdin_until_newline();
                sleep(1);
                break;
        }
    }
}

int main(int argc, char **argv) {
    struct options opts;
    if (argc < 2) {
        return interactive_menu(argv[0]);
    }
    if (parse_options(argc, argv, &opts) != 0) {
        print_usage(argv[0]);
        return 1;
    }
    if (strcmp(opts.command, "help") == 0) {
        print_usage(argv[0]);
        return 0;
    }
    if (strcmp(opts.command, "init") == 0) {
        return command_init();
    }
    if (strcmp(opts.command, "useradd") == 0) {
        return command_useradd(&opts);
    }
    if (strcmp(opts.command, "userdel") == 0) {
        return command_userdel(&opts);
    }
    if (strcmp(opts.command, "usermod") == 0) {
        return command_usermod(&opts);
    }
    if (strcmp(opts.command, "passwd") == 0) {
        return command_passwd(&opts);
    }
    if (strcmp(opts.command, "userlist") == 0) {
        return command_userlist();
    }
    if (strcmp(opts.command, "grouplist") == 0) {
        return command_grouplist();
    }
    print_usage(argv[0]);
    return 1;
}
