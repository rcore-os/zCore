/* apk_provider_data.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2012 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include "apk_defines.h"
#include "apk_blob.h"

struct apk_provider {
	struct apk_package *pkg;
	apk_blob_t *version;
};
APK_ARRAY(apk_provider_array, struct apk_provider);

#define PROVIDER_FMT		"%s%s"BLOB_FMT
#define PROVIDER_PRINTF(n,p)	(n)->name, (p)->version->len ? "-" : "", BLOB_PRINTF(*(p)->version)
