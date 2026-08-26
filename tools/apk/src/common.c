/* common.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <string.h>
#include <stdlib.h>
#include <unistd.h>
#include "apk_defines.h"
#include "apk_balloc.h"

const struct apk_array _apk_array_empty = { .num = 0 };

void *_apk_array_resize(struct apk_array *array, size_t item_size, size_t num, size_t cap)
{
	uint32_t old_num;

	if (cap == 0) {
		_apk_array_free(array);
		return (void*) &_apk_array_empty;
	}
	if (num > cap) num = cap;
	old_num = array->num;

	if (!array->allocated || cap != array->capacity) {
		if (!array->allocated) array = NULL;
		array = realloc(array, sizeof(struct apk_array) + cap * item_size);
	}
	*array = (struct apk_array) {
		.num = num,
		.capacity = cap,
		.allocated = 1,
	};
	if (unlikely(old_num < num)) memset(((void*)(array+1)) + item_size * old_num, 0, item_size * (num - old_num));
	return array;
}

void *_apk_array_copy(struct apk_array *dst, const struct apk_array *src, size_t item_size)
{
	if (dst == src) return dst;
	struct apk_array *copy = _apk_array_resize(dst, item_size, 0, max(src->num, dst->capacity));
	if (src->num != 0) {
		memcpy(copy+1, src+1, item_size * src->num);
		copy->num = src->num;
	}
	return copy;
}

void *_apk_array_grow(struct apk_array *array, size_t item_size)
{
	return _apk_array_resize(array, item_size, array->num, array->capacity + min(array->capacity + 2, 64));
}

void _apk_array__free(const struct apk_array *array)
{
	free((void*) array);
}

void *_apk_array_balloc(const struct apk_array *array, size_t item_size, size_t capacity, struct apk_balloc *ba)
{
	_apk_array_free(array);

	struct apk_array *n = apk_balloc_new_extra(ba, struct apk_array, capacity * item_size);
	if (!n) return (void*) &_apk_array_empty;
	*n = (struct apk_array) {
		.num = 0,
		.capacity = capacity,
	};
	return n;
}

void *_apk_array_bclone(struct apk_array *array, size_t item_size, struct apk_balloc *ba)
{
	if (!array->allocated) return array;
	if (array->num == 0) return (void*) &_apk_array_empty;
	uint32_t num = array->num;
	size_t sz = num * item_size;
	struct apk_array *n = apk_balloc_new_extra(ba, struct apk_array, sz);
	*n = (struct apk_array) {
		.capacity = num,
		.num = num,
	};
	memcpy(n+1, array+1, sz);
	return n;
}

int apk_string_array_qsort(const void *a, const void *b)
{
	return strcmp(*(const char **)a, *(const char **)b);
}

time_t apk_get_build_time(time_t mtime)
{
	static int initialized = 0;
	static time_t timestamp = 0;

	if (!initialized) {
		char *source_date_epoch = getenv("SOURCE_DATE_EPOCH");
		initialized = 1;
		if (source_date_epoch && *source_date_epoch) {
			timestamp = strtoull(source_date_epoch, NULL, 10);
			initialized = 2;
		}
	}
	if (initialized == 2) return timestamp;
	return mtime;
}
