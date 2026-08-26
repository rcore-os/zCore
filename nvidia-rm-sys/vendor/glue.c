/*
 * OUR code, not NVIDIA's -- a tiny shim for the handful of real NVIDIA
 * symbols that are variadic C functions. Stable Rust cannot export a
 * variadic `extern "C" fn` (only import one), so the variadic side has to
 * live in C; this formats the message with a minimal freestanding
 * printf-style formatter and forwards the RESULT to a fixed-arity
 * Rust-exported logger.
 *
 * The formatter exists because the original version forwarded only the
 * raw format string, dropping all arguments -- which turned the first
 * real diagnostic NVIDIA's RM core ever printed on real hardware into a
 * literal "%.*s" on the console at the exact moment the machine hung.
 * The dropped argument WAS the diagnosis. Never again: this supports the
 * conversions RM diagnostics actually use (%s, %.*s, %c, %d/%i, %u,
 * %x/%X, %p, %% with l/ll/z/h length modifiers and basic width/zero-pad)
 * -- not a full printf, but every dropped-conversion case still prints
 * the raw conversion spec so nothing disappears silently.
 *
 * nv_printf is os-interface.h's real printf entry point (see
 * arch/nvalloc/unix/include/os-interface.h) -- the real backend for
 * nvDbg_Printf (nvlog_printf.c) and portDbgPrintString, i.e. every
 * NV_PRINTF and nvport assert message in the vendored core funnels
 * through here.
 */

#include <stdarg.h>

extern void nvrm_shim_log_raw(const char *msg);

#define NV_PRINTF_BUF_CAP 512u

static void emit(char *buf, unsigned *pos, char c)
{
    if (*pos + 1u < NV_PRINTF_BUF_CAP)
    {
        buf[(*pos)++] = c;
    }
}

/* Emits `value` in `base` (10 or 16), honoring sign/width/zero-pad. */
static void emit_num(char *buf, unsigned *pos, unsigned long long value,
                     int negative, unsigned base, int upper,
                     unsigned width, int zero_pad)
{
    char tmp[24]; /* 64-bit max: 20 decimal digits or 16 hex digits */
    unsigned n = 0;
    static const char lo[] = "0123456789abcdef";
    static const char hi[] = "0123456789ABCDEF";
    const char *digits = upper ? hi : lo;

    do
    {
        tmp[n++] = digits[value % base];
        value /= base;
    } while (value != 0 && n < sizeof(tmp));

    if (negative)
    {
        tmp[n++] = '-';
    }

    while (width > n)
    {
        emit(buf, pos, zero_pad ? '0' : ' ');
        width--;
    }
    while (n > 0)
    {
        emit(buf, pos, tmp[--n]);
    }
}

static void vformat(char *buf, unsigned *pos, const char *fmt, va_list ap)
{
    const char *p;

    for (p = fmt; *p != '\0'; p++)
    {
        if (*p != '%')
        {
            emit(buf, pos, *p);
            continue;
        }

        /* conversion spec: %[-0][width][.precision][length]conv */
        {
            const char *spec_start = p; /* points at '%' */
            unsigned width = 0;
            int precision = -1; /* -1 = unspecified */
            int zero_pad = 0;
            int longs = 0; /* count of 'l'; 'z' treated as 2 on x86_64 */

            p++;
            if (*p == '%')
            {
                emit(buf, pos, '%');
                continue;
            }

            /* flags (only the ones that matter for our output) */
            while (*p == '-' || *p == '0' || *p == '+' || *p == ' ' || *p == '#')
            {
                if (*p == '0')
                {
                    zero_pad = 1;
                }
                p++;
            }
            /* width */
            if (*p == '*')
            {
                int w = va_arg(ap, int);
                width = (w > 0) ? (unsigned)w : 0u;
                p++;
            }
            else
            {
                while (*p >= '0' && *p <= '9')
                {
                    width = width * 10u + (unsigned)(*p - '0');
                    p++;
                }
            }
            /* precision */
            if (*p == '.')
            {
                p++;
                if (*p == '*')
                {
                    precision = va_arg(ap, int);
                    p++;
                }
                else
                {
                    precision = 0;
                    while (*p >= '0' && *p <= '9')
                    {
                        precision = precision * 10 + (*p - '0');
                        p++;
                    }
                }
            }
            /* length modifiers */
            while (*p == 'l' || *p == 'h' || *p == 'z')
            {
                if (*p == 'l' || *p == 'z')
                {
                    longs++;
                }
                /* 'h'/'hh' promote through varargs to int anyway */
                p++;
            }

            switch (*p)
            {
                case 's':
                {
                    const char *s = va_arg(ap, const char *);
                    int i;
                    if (s == 0)
                    {
                        s = "(null)";
                    }
                    for (i = 0; s[i] != '\0'; i++)
                    {
                        if (precision >= 0 && i >= precision)
                        {
                            break;
                        }
                        emit(buf, pos, s[i]);
                    }
                    break;
                }
                case 'c':
                {
                    char c = (char)va_arg(ap, int);
                    emit(buf, pos, c);
                    break;
                }
                case 'd':
                case 'i':
                {
                    long long v = (longs >= 2) ? va_arg(ap, long long)
                                : (longs == 1) ? (long long)va_arg(ap, long)
                                               : (long long)va_arg(ap, int);
                    unsigned long long mag =
                        (v < 0) ? (unsigned long long)(-(v + 1)) + 1ull
                                : (unsigned long long)v;
                    emit_num(buf, pos, mag, v < 0, 10u, 0, width, zero_pad);
                    break;
                }
                case 'u':
                {
                    unsigned long long v = (longs >= 2) ? va_arg(ap, unsigned long long)
                                        : (longs == 1) ? (unsigned long long)va_arg(ap, unsigned long)
                                                       : (unsigned long long)va_arg(ap, unsigned int);
                    emit_num(buf, pos, v, 0, 10u, 0, width, zero_pad);
                    break;
                }
                case 'x':
                case 'X':
                {
                    unsigned long long v = (longs >= 2) ? va_arg(ap, unsigned long long)
                                        : (longs == 1) ? (unsigned long long)va_arg(ap, unsigned long)
                                                       : (unsigned long long)va_arg(ap, unsigned int);
                    emit_num(buf, pos, v, 0, 16u, (*p == 'X'), width, zero_pad);
                    break;
                }
                case 'p':
                {
                    void *v = va_arg(ap, void *);
                    emit(buf, pos, '0');
                    emit(buf, pos, 'x');
                    emit_num(buf, pos, (unsigned long long)v, 0, 16u, 0, 0, 0);
                    break;
                }
                default:
                {
                    /*
                     * Unknown conversion: emit the raw spec verbatim so it
                     * is visible (never silently dropped), and stop
                     * consuming args for it (safest possible guess).
                     */
                    const char *q;
                    for (q = spec_start; q <= p && *q != '\0'; q++)
                    {
                        emit(buf, pos, *q);
                    }
                    break;
                }
            }
        }
    }
}

int nv_printf(unsigned int debuglevel, const char *printf_format, ...)
{
    char buf[NV_PRINTF_BUF_CAP];
    unsigned pos = 0;
    va_list ap;

    (void)debuglevel;

    va_start(ap, printf_format);
    vformat(buf, &pos, printf_format, ap);
    va_end(ap);

    /* Trim trailing newlines: the Rust logger adds its own line framing. */
    while (pos > 0 && (buf[pos - 1] == '\n' || buf[pos - 1] == '\r'))
    {
        pos--;
    }
    buf[pos] = '\0';

    if (pos > 0)
    {
        nvrm_shim_log_raw(buf);
    }
    return (int)pos;
}
