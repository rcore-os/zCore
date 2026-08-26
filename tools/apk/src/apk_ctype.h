/* apk_ctype.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2024 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once

enum {
	APK_CTYPE_HEXDIGIT = 0,
	APK_CTYPE_PACKAGE_NAME,
	APK_CTYPE_VERSION_SUFFIX,
	APK_CTYPE_DEPENDENCY_NAME,
	APK_CTYPE_DEPENDENCY_COMPARER,
	APK_CTYPE_VARIABLE_NAME,
	APK_CTYPE_TAG_NAME,

	APK_CTYPE_DEPENDENCY_SEPARATOR = 8,
	APK_CTYPE_REPOSITORY_SEPARATOR,
};

int apk_blob_spn(apk_blob_t blob, unsigned char ctype, apk_blob_t *l, apk_blob_t *r);
int apk_blob_cspn(apk_blob_t blob, unsigned char ctype, apk_blob_t *l, apk_blob_t *r);
