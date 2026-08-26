/* balloc.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2024 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <stdlib.h>
#include "apk_defines.h"
#include "apk_balloc.h"

struct apk_balloc_page {
	struct hlist_node pages_list;
};

void apk_balloc_init(struct apk_balloc *ba, size_t page_size)
{
	*ba = (struct apk_balloc) { .page_size = page_size };
}

void apk_balloc_destroy(struct apk_balloc *ba)
{
	struct apk_balloc_page *p;
	struct hlist_node *pn, *pc;

	hlist_for_each_entry_safe(p, pc, pn, &ba->pages_head, pages_list)
		free(p);
	memset(ba, 0, sizeof *ba);
}

void *apk_balloc_aligned(struct apk_balloc *ba, size_t size, size_t align)
{
	uintptr_t ptr = ROUND_UP(ba->cur, align);
	if (ptr + size > ba->end) {
		size_t page_size = max(ba->page_size, size);
		struct apk_balloc_page *bp = malloc(page_size + sizeof(struct apk_balloc_page));
		hlist_add_head(&bp->pages_list, &ba->pages_head);
		ba->cur = (intptr_t)bp + sizeof *bp;
		ba->end = (intptr_t)bp + page_size;
		ptr = ROUND_UP(ba->cur, align);
	}
	ba->cur = ptr + size;
	return (void *) ptr;
}

void *apk_balloc_aligned0(struct apk_balloc *ba, size_t size, size_t align)
{
	void *ptr = apk_balloc_aligned(ba, size, align);
	memset(ptr, 0, size);
	return ptr;
}

apk_blob_t apk_balloc_dup(struct apk_balloc *ba, apk_blob_t b)
{
	void *ptr = apk_balloc_aligned(ba, b.len, 1);
	memcpy(ptr, b.ptr, b.len);
	return APK_BLOB_PTR_LEN(ptr, b.len);
}

char *apk_balloc_cstr(struct apk_balloc *ba, apk_blob_t b)
{
	char *str = apk_balloc_aligned(ba, b.len + 1, 1);
	memcpy(str, b.ptr, b.len);
	str[b.len] = 0;
	return str;
}
