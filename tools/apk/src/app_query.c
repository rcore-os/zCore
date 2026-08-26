/* app_query.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2025 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <unistd.h>
#include "apk_database.h"
#include "apk_applet.h"
#include "apk_query.h"

static int query_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	return apk_query_main(ac, args);
}

static struct apk_applet apk_query = {
	.name = "query",
	.optgroup_query = 1,
	.open_flags = APK_OPENF_READ | APK_OPENF_ALLOW_ARCH,
	.main = query_main,
};

APK_DEFINE_APPLET(apk_query);
