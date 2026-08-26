/* apk_pathbuilder.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2021 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include "apk_defines.h"
#include "apk_blob.h"

struct apk_pathbuilder {
	uint16_t namelen;
	char name[PATH_MAX];
};

int apk_pathbuilder_pushb(struct apk_pathbuilder *pb, apk_blob_t b);
void apk_pathbuilder_pop(struct apk_pathbuilder *pb, int);


static inline int apk_pathbuilder_setb(struct apk_pathbuilder *pb, apk_blob_t b)
{
	pb->namelen = 0;
	return apk_pathbuilder_pushb(pb, b);
}

static inline int apk_pathbuilder_push(struct apk_pathbuilder *pb, const char *name)
{
	return apk_pathbuilder_pushb(pb, APK_BLOB_STR(name));
}

static inline const char *apk_pathbuilder_cstr(const struct apk_pathbuilder *pb)
{
	return pb->name;
}

static inline apk_blob_t apk_pathbuilder_get(const struct apk_pathbuilder *pb)
{
	return APK_BLOB_PTR_LEN((void*)pb->name, pb->namelen);
}
