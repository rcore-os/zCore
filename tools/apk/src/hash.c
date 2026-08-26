/* hash.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include "apk_defines.h"
#include "apk_hash.h"

void apk_hash_init(struct apk_hash *h, const struct apk_hash_ops *ops,
		   int num_buckets)
{
	h->ops = ops;
	apk_hash_array_init(&h->buckets);
	apk_hash_array_resize(&h->buckets, num_buckets, num_buckets);
	h->num_items = 0;
}

static int apk_hash_free_item_enumerator(apk_hash_item item, void *ctx)
{
	((apk_hash_delete_f) ctx)(item);
	return 0;
}

void apk_hash_free(struct apk_hash *h)
{
	if (h->ops->delete_item) apk_hash_foreach(h, apk_hash_free_item_enumerator, h->ops->delete_item);
	apk_hash_array_free(&h->buckets);
}

int apk_hash_foreach(struct apk_hash *h, apk_hash_enumerator_f e, void *ctx)
{
	apk_hash_node *pos, *n;
	ptrdiff_t offset = h->ops->node_offset;
	int r;

	apk_array_foreach(bucket, h->buckets) {
		hlist_for_each_safe(pos, n, bucket) {
			r = e(((void *) pos) - offset, ctx);
			if (r != 0) return r;
		}
	}

	return 0;
}

apk_hash_item apk_hash_get_hashed(struct apk_hash *h, apk_blob_t key, unsigned long hash)
{
	ptrdiff_t offset = h->ops->node_offset;
	apk_hash_node *pos;
	apk_hash_item item;
	apk_blob_t itemkey;

	hash %= apk_array_len(h->buckets);
	if (h->ops->compare_item != NULL) {
		hlist_for_each(pos, &h->buckets->item[hash]) {
			item = ((void *) pos) - offset;
			if (h->ops->compare_item(item, key) == 0)
				return item;
		}
	} else {
		hlist_for_each(pos, &h->buckets->item[hash]) {
			item = ((void *) pos) - offset;
			itemkey = h->ops->get_key(item);
			if (h->ops->compare(key, itemkey) == 0)
				return item;
		}
	}

	return NULL;
}

void apk_hash_insert_hashed(struct apk_hash *h, apk_hash_item item, unsigned long hash)
{
	apk_hash_node *node;

	hash %= apk_array_len(h->buckets);
	node = (apk_hash_node *) (item + h->ops->node_offset);
	hlist_add_head(node, &h->buckets->item[hash]);
	h->num_items++;
}

void apk_hash_delete_hashed(struct apk_hash *h, apk_blob_t key, unsigned long hash)
{
	ptrdiff_t offset = h->ops->node_offset;
	apk_hash_node *pos;
	apk_hash_item item;

	assert(h->ops->compare_item != NULL);

	hash %= apk_array_len(h->buckets);
	hlist_for_each(pos, &h->buckets->item[hash]) {
		item = ((void *) pos) - offset;
		if (h->ops->compare_item(item, key) == 0) {
			hlist_del(pos, &h->buckets->item[hash]);
			if (h->ops->delete_item) h->ops->delete_item(item);
			h->num_items--;
			break;
		}
	}
}
