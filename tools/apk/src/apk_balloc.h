/* apk_balloc.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2024 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include "apk_defines.h"
#include "apk_blob.h"

struct apk_balloc {
	struct hlist_head pages_head;
	size_t page_size;
	uintptr_t cur, end;
};

void apk_balloc_init(struct apk_balloc *ba, size_t page_size);
void apk_balloc_destroy(struct apk_balloc *ba);
void *apk_balloc_aligned(struct apk_balloc *ba, size_t size, size_t align);
void *apk_balloc_aligned0(struct apk_balloc *ba, size_t size, size_t align);
apk_blob_t apk_balloc_dup(struct apk_balloc *ba, apk_blob_t b);
char *apk_balloc_cstr(struct apk_balloc *ba, apk_blob_t b);

#define apk_balloc_new_extra(ba, type, extra) (type *) apk_balloc_aligned(ba, sizeof(type)+extra, alignof(type))
#define apk_balloc_new(ba, type) (type *) apk_balloc_new_extra(ba, type, 0)
#define apk_balloc_new0_extra(ba, type, extra) (type *) apk_balloc_aligned0(ba, sizeof(type)+extra, alignof(type))
#define apk_balloc_new0(ba, type) (type *) apk_balloc_new0_extra(ba, type, 0)
