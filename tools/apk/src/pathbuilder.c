/* apk_pathbuilder.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2021 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <errno.h>
#include "apk_pathbuilder.h"

int apk_pathbuilder_pushb(struct apk_pathbuilder *pb, apk_blob_t b)
{
	size_t oldlen = pb->namelen, i = pb->namelen;
	if (i + b.len + 2 >= ARRAY_SIZE(pb->name)) return -ENAMETOOLONG;
	if (i) pb->name[i++] = '/';
	memcpy(&pb->name[i], b.ptr, b.len);
	pb->namelen = i + b.len;
	pb->name[pb->namelen] = 0;
	return oldlen;
}

void apk_pathbuilder_pop(struct apk_pathbuilder *pb, int pos)
{
	if (pos < 0) return;
	pb->namelen = pos;
	pb->name[pb->namelen] = 0;
}
