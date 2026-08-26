/* help.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2020 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <zlib.h>
#include "apk_applet.h"
#include "apk_print.h"
#include "help.h"

static LIST_HEAD(apk_applet_list);

#define apk_applet_foreach(iter) list_for_each_entry(iter, &apk_applet_list, node)

void apk_applet_register(struct apk_applet *applet)
{
	list_init(&applet->node);
	list_add_tail(&applet->node, &apk_applet_list);
}

struct apk_applet *apk_applet_find(const char *name)
{
	struct apk_applet *a;

	apk_applet_foreach(a) {
		if (strcmp(name, a->name) == 0)
			return a;
	}
	return NULL;
}

#ifndef NO_HELP
static inline int is_group(struct apk_applet *applet, const char *topic)
{
	if (!applet) return strcmp(topic, "APK") == 0;
	if (strcmp(topic, applet->name) == 0) return 1;
	if (strcmp(topic, "GLOBAL") == 0) return 1;
	if (applet->optgroup_generation && strcmp(topic, "GENERATION") == 0) return 1;
	if (applet->optgroup_commit && strcmp(topic, "COMMIT") == 0) return 1;
	if (applet->optgroup_query && strcmp(topic, "QUERY") == 0) return 1;
	return 0;
}

static bool decompress_help(char *buf, size_t bufsz)
{
	z_stream strm = {
		.avail_in = sizeof compressed_help,
		.next_in = (unsigned char *) compressed_help,
		.avail_out = bufsz,
		.next_out = (unsigned char *) buf,
	};
	/* Use inflateInit2 with windowBits=47 (15+32) to auto-detect gzip or zlib format */
	int ret = inflateInit2(&strm, 15 + 32);
	if (ret != Z_OK) return false;
	ret = inflate(&strm, Z_FINISH);
	inflateEnd(&strm);
	return ret == Z_STREAM_END && strm.total_out == bufsz;
}

void apk_applet_help(struct apk_applet *applet, struct apk_out *out)
{
	char buf[uncompressed_help_size];
	int num = 0;

	if (!decompress_help(buf, sizeof buf)) {
		apk_err(out, "Help decompression failed");
		return;
	}

	for (const char *ptr = buf, *msg; *ptr && ptr < &buf[sizeof buf]; ptr = msg + strlen(msg) + 1) {
		msg = ptr + strlen(ptr) + 1;
		if (is_group(applet, ptr)) {
			fputc('\n', stdout);
			fwrite(msg, strlen(msg), 1, stdout);
			num++;
		}
	}
	if (num == 0) apk_err(out, "Help not found");
}
#else
void apk_applet_help(struct apk_applet *applet, struct apk_out *out)
{
	fputc('\n', stdout);
	apk_err(out, "This apk-tools has been built without help");
}
#endif
