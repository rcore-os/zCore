/* app_stats.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2013 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <stdio.h>
#include "apk_defines.h"
#include "apk_applet.h"
#include "apk_database.h"

static int list_count(struct list_head *h)
{
	struct list_head *n;
	int c = 0;

	list_for_each(n, h)
		c++;

	return c;
}

static int stats_main(void *ctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_out *out = &ac->out;
	struct apk_database *db = ac->db;

	apk_out(out,
		"installed:\n"
		"  packages: %d\n"
		"  dirs: %d\n"
		"  files: %d\n"
		"  bytes: %" PRIu64 "\n"
		"  triggers: %d\n"
		"available:\n"
		"  names: %d\n"
		"  packages: %d\n"
		"atoms:\n"
		"  num: %d\n"
		,
		db->installed.stats.packages,
		db->installed.stats.dirs,
		db->installed.stats.files,
		db->installed.stats.bytes,
		list_count(&db->installed.triggers),
		db->available.names.num_items,
		db->available.packages.num_items,
		db->atoms.hash.num_items
		);
	return 0;
}

static struct apk_applet stats_applet = {
	.name = "stats",
	.open_flags = APK_OPENF_READ,
	.main = stats_main,
};

APK_DEFINE_APPLET(stats_applet);


