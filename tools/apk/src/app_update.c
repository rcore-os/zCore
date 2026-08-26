/* app_update.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <stdio.h>
#include "apk_defines.h"
#include "apk_applet.h"
#include "apk_database.h"
#include "apk_version.h"
#include "apk_print.h"

static int update_parse_options(void *ctx, struct apk_ctx *ac, int opt, const char *optarg)
{
	switch (opt) {
	case APK_OPTIONS_INIT:
		ac->cache_max_age = 0;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static int update_main(void *ctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_out *out = &ac->out;
	struct apk_database *db = ac->db;
	const char *msg = "OK:";
	char buf[64];
	int r = db->repositories.unavailable + db->repositories.stale;

	if (db->idb_dirty && apk_db_write_config(db) != 0) r++;

	if (apk_out_verbosity(out) < 1) return r;

	apk_db_foreach_repository(repo, db) {
		if (!repo->available) continue;
		apk_msg(out, BLOB_FMT " [" BLOB_FMT "]",
			BLOB_PRINTF(repo->description),
			BLOB_PRINTF(repo->url_printable));
	}

	if (db->repositories.unavailable || db->repositories.stale)
		msg = apk_fmts(buf, sizeof buf, "%d unavailable, %d stale;",
			 db->repositories.unavailable,
			 db->repositories.stale) ?: "ERRORS;";

	apk_msg(out, "%s %d distinct packages available", msg,
		db->available.packages.num_items);
	return r;
}

static struct apk_applet apk_update = {
	.name = "update",
	.open_flags = APK_OPENF_WRITE | APK_OPENF_ALLOW_ARCH,
	.parse = update_parse_options,
	.main = update_main,
};

APK_DEFINE_APPLET(apk_update);

