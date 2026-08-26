/* apk_process.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2008-2024 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include <sys/types.h>
#include "apk_defines.h"
#include "apk_blob.h"

struct apk_out;
struct apk_istream;

struct apk_process {
	int pipe_stdin[2], pipe_stdout[2], pipe_stderr[2];
	pid_t pid;
	const char *linepfx, *logpfx, *argv0;
	struct apk_out *out;
	struct apk_istream *is;
	apk_blob_t is_blob;
	int status;
	unsigned int is_eof : 1;
	struct buf {
		uint16_t len;
		uint8_t buf[1022];
	} buf_stdout, buf_stderr;
};

int apk_process_init(struct apk_process *p, const char *argv0, const char *logpfx, struct apk_out *out, struct apk_istream *is);
pid_t apk_process_fork(struct apk_process *p);
int apk_process_spawn(struct apk_process *p, const char *path, char * const* argv, char * const* env);
int apk_process_run(struct apk_process *p);
int apk_process_cleanup(struct apk_process *p);
struct apk_istream *apk_process_istream(char * const* argv, struct apk_out *out, const char *argv0);
