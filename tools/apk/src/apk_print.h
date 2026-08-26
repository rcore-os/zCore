/* apk_print.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include <stdio.h>
#include "apk_blob.h"
#include "apk_io.h"

struct apk_out;
struct apk_progress;

const char *apk_error_str(int error);
const char *apk_last_path_segment(const char *);
int apk_get_human_size_unit(apk_blob_t b);
apk_blob_t apk_fmt_human_size(char *buf, size_t sz, uint64_t val, int pretty_print);
apk_blob_t apk_url_sanitize(apk_blob_t url, struct apk_balloc *ba);

struct apk_out {
	int verbosity, progress_fd;
	unsigned int width;
	unsigned int progress : 2;
	unsigned int need_flush : 1;
	const char *progress_char;
	FILE *out, *err, *log;
	struct apk_progress *prog;
};

static inline int apk_out_verbosity(struct apk_out *out) { return out->verbosity; }

// Pass this as the prefix to skip logging to the console (but still write to
// the log file).
#define APK_OUT_LOG_ONLY ((const char*)-1)
#define APK_OUT_ERROR    "ERROR: "
#define APK_OUT_WARNING  "WARNING: "
#define APK_OUT_FLUSH    ""

#define apk_err(out, args...)	do { apk_out_fmt(out, APK_OUT_ERROR, args); } while (0)
#define apk_out(out, args...)	do { apk_out_fmt(out, NULL, args); } while (0)
#define apk_warn(out, args...)	do { if (apk_out_verbosity(out) >= 0) { apk_out_fmt(out, APK_OUT_WARNING, args); } } while (0)
#define apk_notice(out, args...) do { if (apk_out_verbosity(out) >= 1) { apk_out_fmt(out, APK_OUT_FLUSH, args); } } while (0)
#define apk_msg(out, args...)	do { if (apk_out_verbosity(out) >= 1) { apk_out_fmt(out, NULL, args); } } while (0)
#define apk_dbg(out, args...)	do { if (apk_out_verbosity(out) >= 2) { apk_out_fmt(out, NULL, args); } } while (0)
#define apk_dbg2(out, args...)	do { if (apk_out_verbosity(out) >= 3) { apk_out_fmt(out, NULL, args); } } while (0)

void apk_out_configure_progress(struct apk_out *out, bool on_tty);
void apk_out_reset(struct apk_out *);
void apk_out_progress_note(struct apk_out *out, const char *format, ...)
	__attribute__ ((format (printf, 2, 3)));
void apk_out_fmt(struct apk_out *, const char *prefix, const char *format, ...)
	__attribute__ ((format (printf, 3, 4)));
void apk_out_log_argv(struct apk_out *, char **argv);

struct apk_progress {
	struct apk_out *out;
	const char *stage;
	int last_bar, last_percent;
	uint64_t cur_progress, max_progress;
	uint64_t item_base_progress, item_max_progress;
};

uint64_t apk_progress_weight(uint64_t bytes, unsigned int packages);
void apk_progress_start(struct apk_progress *p, struct apk_out *out, const char *stage, uint64_t max_progress);
void apk_progress_update(struct apk_progress *p, uint64_t cur_progress);
void apk_progress_end(struct apk_progress *p);
void apk_progress_item_start(struct apk_progress *p, uint64_t base_progress, uint64_t max_item_progress);
void apk_progress_item_end(struct apk_progress *p);

struct apk_progress_istream {
	struct apk_istream is;
	struct apk_istream *pis;
	struct apk_progress *p;
	uint64_t done;
};
struct apk_istream *apk_progress_istream(struct apk_progress_istream *pis, struct apk_istream *is, struct apk_progress *p);

struct apk_indent {
	struct apk_out *out;
	unsigned int x, indent, err;
};

void apk_print_indented_init(struct apk_indent *i, struct apk_out *out, int err);
void apk_print_indented_line(struct apk_indent *i, const char *fmt, ...);
void apk_print_indented_group(struct apk_indent *i, int indent, const char *fmt, ...);
void apk_print_indented_end(struct apk_indent *i);
int  apk_print_indented(struct apk_indent *i, apk_blob_t blob);
void apk_print_indented_words(struct apk_indent *i, const char *text);
void apk_print_indented_fmt(struct apk_indent *i, const char *fmt, ...)
	__attribute__ ((format (printf, 2, 3)));
