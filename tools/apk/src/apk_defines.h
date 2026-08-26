/* apk_defines.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include <assert.h>
#include <endian.h>
#include <limits.h>
#include <inttypes.h>
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <string.h>
#include <errno.h>
#include <time.h>

#define ARRAY_SIZE(x)	(sizeof(x) / sizeof((x)[0]))
#define BIT(x)		(1ULL << (x))
#define min(a, b)	((a) < (b) ? (a) : (b))
#define max(a, b)	((a) > (b) ? (a) : (b))

#ifndef NULL
#define NULL 0L
#endif

#define APK_NO		0
#define APK_YES		1
#define APK_AUTO	2

enum {
	APKE_FIRST_VALUE = 1024,

	APKE_FILE_UNCHANGED = APKE_FIRST_VALUE,
	APKE_EOF,
	APKE_DNS_FAIL,
	APKE_DNS_ADDRESS_FAMILY,
	APKE_DNS_AGAIN,
	APKE_DNS_NO_DATA,
	APKE_DNS_NO_NAME,
	APKE_TLS_ERROR,
	APKE_TLS_SERVER_CERT_HOSTNAME,
	APKE_TLS_SERVER_CERT_UNTRUSTED,
	APKE_TLS_CLIENT_CERT_UNTRUSTED,
	APKE_TLS_HANDSHAKE,
	APKE_URL_FORMAT,
	APKE_HTTP_400_BAD_REQUEST,
	APKE_HTTP_401_UNAUTHORIZED,
	APKE_HTTP_403_FORBIDDEN,
	APKE_HTTP_404_NOT_FOUND,
	APKE_HTTP_405_METHOD_NOT_ALLOWED,
	APKE_HTTP_406_NOT_ACCEPTABLE,
	APKE_HTTP_407_PROXY_AUTH_REQUIRED,
	APKE_HTTP_408_TIMEOUT,
	APKE_HTTP_500_INTERNAL_SERVER_ERROR,
	APKE_HTTP_501_NOT_IMPLEMENTED,
	APKE_HTTP_502_BAD_GATEWAY,
	APKE_HTTP_503_SERVICE_UNAVAILABLE,
	APKE_HTTP_504_GATEWAY_TIMEOUT,
	APKE_HTTP_UNKNOWN,
	APKE_CRYPTO_ERROR,
	APKE_CRYPTO_NOT_SUPPORTED,
	APKE_CRYPTO_KEY_FORMAT,
	APKE_SIGNATURE_GEN_FAILURE,
	APKE_SIGNATURE_UNTRUSTED,
	APKE_SIGNATURE_INVALID,
	APKE_FORMAT_INVALID,
	APKE_FORMAT_OBSOLETE,
	APKE_FORMAT_NOT_SUPPORTED,
	APKE_PKGNAME_FORMAT,
	APKE_PKGVERSION_FORMAT,
	APKE_DEPENDENCY_FORMAT,
	APKE_ADB_COMPRESSION,
	APKE_ADB_HEADER,
	APKE_ADB_VERSION,
	APKE_ADB_SCHEMA,
	APKE_ADB_BLOCK,
	APKE_ADB_SIGNATURE,
	APKE_ADB_INTEGRITY,
	APKE_ADB_NO_FROMSTRING,
	APKE_ADB_LIMIT,
	APKE_ADB_PACKAGE_FORMAT,
	APKE_V2DB_FORMAT,
	APKE_V2PKG_FORMAT,
	APKE_V2PKG_INTEGRITY,
	APKE_V2NDX_FORMAT,
	APKE_PACKAGE_NOT_FOUND,
	APKE_PACKAGE_NAME_SPEC,
	APKE_INDEX_STALE,
	APKE_FILE_INTEGRITY,
	APKE_CACHE_NOT_AVAILABLE,
	APKE_UVOL_NOT_AVAILABLE,
	APKE_UVOL_ERROR,
	APKE_UVOL_ROOT,
	APKE_REMOTE_IO,
	APKE_NOT_EXTRACTED,
	APKE_REPO_SYNTAX,
	APKE_REPO_KEYWORD,
	APKE_REPO_VARIABLE,
	APKE_BUFFER_SIZE,
};

static inline void *ERR_PTR(long error) { return (void*) error; }
static inline void *ERR_CAST(const void *ptr) { return (void*) ptr; }
static inline int PTR_ERR(const void *ptr) { return (int)(long) ptr; }
static inline int IS_ERR(const void *ptr) { return (unsigned long)ptr >= (unsigned long)-4095; }

#ifndef __has_builtin
#define __has_builtin(x) 0
#endif

#if !__has_builtin(__builtin_expect)
#define __builtin_expect(x, expected_value) (x)
#endif

#ifndef likely
#define likely(x) __builtin_expect((!!(x)),1)
#endif

#ifndef unlikely
#define unlikely(x) __builtin_expect((!!(x)),0)
#endif

#ifndef typeof
#define typeof(x) __typeof__(x)
#endif

#ifndef alignof
#define alignof(x) _Alignof(x)
#endif

#ifndef container_of
#define container_of(ptr, type, member) ({                      \
        const typeof( ((type *)0)->member ) *__mptr = (ptr);    \
        (type *)( (char *)__mptr - offsetof(type,member) );})
#endif

#define ROUND_DOWN(x,a)		((x) & ~(a-1))
#define ROUND_UP(x,a)		(((x)+(a)-1) & ~((a)-1))

#define APK_MAX_SCRIPT_SIZE	262144	/* package install script size 256kb */
#define APK_MAX_REPOS		32	/* see struct apk_package */
#define APK_MAX_TAGS		16	/* see solver; unsigned short */

static inline uint64_t apk_calc_installed_size(uint64_t size)
{
	return ROUND_UP(size, 4096ULL);
}

static inline uint16_t apk_unaligned_le16(const void *ptr)
{
	struct unaligned16 { uint16_t value; } __attribute__((packed));
	return le16toh(((struct unaligned16 *) ptr)->value);
}

static inline uint32_t apk_unaligned_le32(const void *ptr)
{
	struct unaligned32 { uint32_t value; } __attribute__((packed));
	return le32toh(((struct unaligned32 *) ptr)->value);
}

static inline uint64_t apk_unaligned_le64(const void *ptr)
{
	struct unaligned64 { uint64_t value; } __attribute__((packed));
	return le64toh(((struct unaligned64 *) ptr)->value);
}

static inline uint64_t apk_aligned32_le64(const void *ptr)
{
	struct unaligned64 { uint64_t value; } __attribute__((aligned(4)));
	return le64toh(((struct unaligned64 *) ptr)->value);
}

time_t apk_get_build_time(time_t);

struct apk_array {
	uint32_t num;
	uint32_t capacity : 31;
	uint32_t allocated : 1;
} __attribute__((aligned(sizeof(void *))));

extern const struct apk_array _apk_array_empty;

void *_apk_array_resize(struct apk_array *hdr, size_t item_size, size_t num, size_t cap);
void *_apk_array_copy(struct apk_array *dst, const struct apk_array *src, size_t item_size);
void *_apk_array_grow(struct apk_array *hdr, size_t item_size);
void _apk_array__free(const struct apk_array *hdr);

struct apk_balloc;
void *_apk_array_balloc(const struct apk_array *hdr, size_t item_size, size_t cap, struct apk_balloc *ba);
void *_apk_array_bclone(struct apk_array *hdr, size_t item_size, struct apk_balloc *ba);

static inline uint32_t _apk_array_len(const struct apk_array *hdr) { return hdr->num; }
static inline void _apk_array_free(const struct apk_array *hdr) {
	if (hdr->allocated) _apk_array__free(hdr);
}
static inline struct apk_array *_apk_array_truncate(struct apk_array *hdr, size_t num) {
	assert(num <= hdr->num);
	if (hdr->num != num) hdr->num = num;
	return hdr;
}

#define apk_array_balloc(array, cap, ba) do { (array) = _apk_array_balloc(&(array)->hdr, apk_array_item_size(array), cap, ba); } while (0)
#define apk_array_bclone(array, ba)	_apk_array_bclone(&(array)->hdr, apk_array_item_size(array), ba)
#define apk_array_len(array)		_apk_array_len(&(array)->hdr)
#define apk_array_truncate(array, num)	_apk_array_truncate(&(array)->hdr, num)
#define apk_array_reset(array)		(typeof(array))((array)->hdr.allocated ? apk_array_truncate(array, 0) : &_apk_array_empty)
#define apk_array_item_size(array)	sizeof((array)->item[0])
#define apk_array_qsort(array, compare)	qsort((array)->item, (array)->hdr.num, apk_array_item_size(array), compare)
#define apk_array_bsearch(array, compare, key)	bsearch(key, (array)->item, (array)->hdr.num, apk_array_item_size(array), compare)

#define APK_ARRAY(array_type_name, item_type_name)			\
	struct array_type_name {					\
		struct apk_array hdr;					\
		item_type_name item[];					\
	};								\
	static inline void						\
	array_type_name##_init(struct array_type_name **a) {		\
		*a = (void *) &_apk_array_empty;			\
	}								\
	static inline void						\
	array_type_name##_free(struct array_type_name **a) {		\
		_apk_array_free(&(*a)->hdr);				\
		*a = (void *) &_apk_array_empty;			\
	}								\
	static inline void						\
	array_type_name##_resize(struct array_type_name **a, size_t num, size_t cap) { \
		*a = _apk_array_resize(&(*a)->hdr, apk_array_item_size(*a), num, cap);\
	}								\
	static inline void						\
	array_type_name##_copy(struct array_type_name **dst, struct array_type_name *src) { \
		*dst = _apk_array_copy(&(*dst)->hdr, &src->hdr, apk_array_item_size(src)); \
	}								\
	static inline item_type_name *					\
	array_type_name##_add(struct array_type_name **a, item_type_name item) {\
		if ((*a)->hdr.num >= (*a)->hdr.capacity) *a = _apk_array_grow(&(*a)->hdr, apk_array_item_size(*a)); \
		item_type_name *nitem = &(*a)->item[((*a)->hdr.num)++];	\
		*nitem = item;						\
		return nitem;						\
	}

APK_ARRAY(apk_string_array, char *);
int apk_string_array_qsort(const void *a, const void *b);

#define apk_array_foreach(__iter, array) \
	for (typeof((array)->item[0]) *__iter = &(array)->item[0], *__end = &__iter[(array)->hdr.num]; __iter < __end; __iter++)
#define apk_array_foreach_item(value, array) \
	for (typeof((array)->item[0]) *__iter = &(array)->item[0], *__end = &__iter[(array)->hdr.num], value; __iter < __end && ({ value = *__iter; 1; }); __iter++)

#define LIST_HEAD(name) struct list_head name = { &name, &name }
#define LIST_END (void *) 0xe01
#define LIST_POISON1 (void *) 0xdeadbeef
#define LIST_POISON2 (void *) 0xabbaabba

struct hlist_node {
	struct hlist_node *next;
};

struct hlist_head {
	struct hlist_node *first;
};

static inline void __hlist_del(struct hlist_node *n, struct hlist_node **pprev)
{
	*pprev = n->next;
	n->next = NULL;
}

static inline void hlist_del(struct hlist_node *n, struct hlist_head *h)
{
	struct hlist_node **pp = &h->first;

	while (*pp != NULL && *pp != LIST_END && *pp != n)
		pp = &(*pp)->next;

	if (*pp == n)
		__hlist_del(n, pp);
}

static inline void hlist_add_head(struct hlist_node *n, struct hlist_head *h)
{
	struct hlist_node *first = h->first;

	n->next = first ? first : LIST_END;
	h->first = n;
}

#define hlist_entry(ptr, type, member) container_of(ptr,type,member)

#define hlist_for_each(pos, head) \
	for (pos = (head)->first; pos && pos != LIST_END; \
	     pos = pos->next)

#define hlist_for_each_safe(pos, n, head) \
	for (pos = (head)->first; pos && pos != LIST_END && \
		({ n = pos->next; 1; }); \
	     pos = n)

#define hlist_for_each_entry(tpos, pos, head, member)			 \
	for (pos = (head)->first;					 \
	     pos && pos != LIST_END  &&					 \
		({ tpos = hlist_entry(pos, typeof(*tpos), member); 1;}); \
	     pos = pos->next)

#define hlist_for_each_entry_safe(tpos, pos, n, head, member) 		 \
	for (pos = (head)->first;					 \
	     pos && pos != LIST_END && ({ n = pos->next; 1; }) && 	 \
		({ tpos = hlist_entry(pos, typeof(*tpos), member); 1;}); \
	     pos = n)


struct list_head {
	struct list_head *next, *prev;
};

static inline void list_init(struct list_head *list)
{
	list->next = list;
	list->prev = list;
}

static inline void __list_add(struct list_head *new,
			      struct list_head *prev,
			      struct list_head *next)
{
	next->prev = new;
	new->next = next;
	new->prev = prev;
	prev->next = new;
}

static inline void list_add_head(struct list_head *new, struct list_head *head)
{
	__list_add(new, head, head->next);
}

static inline void list_add_tail(struct list_head *new, struct list_head *head)
{
	__list_add(new, head->prev, head);
}

static inline void list_add_before(struct list_head *new, struct list_head *other)
{
	__list_add(new, other->prev, other);
}

static inline void __list_del(struct list_head *prev, struct list_head *next)
{
	next->prev = prev;
	prev->next = next;
}

static inline void list_del(struct list_head *entry)
{
	__list_del(entry->prev, entry->next);
	entry->next = LIST_POISON1;
	entry->prev = LIST_POISON2;
}

static inline void list_del_init(struct list_head *entry)
{
	__list_del(entry->prev, entry->next);
	entry->next = NULL;
	entry->prev = NULL;
}

static inline int list_hashed(const struct list_head *n)
{
	return n->next != n && n->next != NULL;
}

static inline int list_empty(const struct list_head *n)
{
	return n->next == n;
}

static inline struct list_head *__list_pop(struct list_head *head)
{
	struct list_head *n = head->next;
	list_del_init(n);
	return n;
}

#define list_entry(ptr, type, member) container_of(ptr,type,member)

#define list_pop(head, type, member) container_of(__list_pop(head),type,member)

#define list_for_each(pos, head) \
	for (pos = (head)->next; pos != (head); pos = pos->next)

#define list_for_each_safe(pos, n, head) \
	for (pos = (head)->next, n = pos->next; pos != (head); \
		pos = n, n = pos->next)

#define list_for_each_entry(pos, head, member)				\
	for (pos = list_entry((head)->next, typeof(*pos), member);	\
	     &pos->member != (head); 					\
	     pos = list_entry(pos->member.next, typeof(*pos), member))

#define list_for_each_entry_safe(pos, n, head, member)			\
	for (pos = list_entry((head)->next, typeof(*pos), member),	\
		n = list_entry(pos->member.next, typeof(*pos), member);	\
	     &pos->member != (head); 					\
	     pos = n, n = list_entry(n->member.next, typeof(*n), member))
