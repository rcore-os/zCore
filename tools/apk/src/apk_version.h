/* apk_version.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include "apk_blob.h"

#define APK_VERSION_UNKNOWN		0
#define APK_VERSION_EQUAL		1
#define APK_VERSION_LESS		2
#define APK_VERSION_GREATER		4
#define APK_VERSION_FUZZY		8
#define APK_VERSION_CONFLICT	16

#define APK_DEPMASK_ANY		(APK_VERSION_EQUAL|APK_VERSION_LESS|\
				 APK_VERSION_GREATER)
#define APK_DEPMASK_CHECKSUM	(APK_VERSION_LESS|APK_VERSION_GREATER)

const char *apk_version_op_string(int op);
int apk_version_result_mask(const char *op);
int apk_version_result_mask_blob(apk_blob_t op);
int apk_version_validate(apk_blob_t ver);
int apk_version_compare(apk_blob_t a, apk_blob_t b);
int apk_version_match(apk_blob_t a, int op, apk_blob_t b);
