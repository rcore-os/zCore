/* apk_crypt.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2021 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include <string.h>
#include "apk_defines.h"
#include "apk_blob.h"

// Digest

#define APK_DIGEST_NONE		0x00
// #define APK_DIGEST_MD5	0x01 // support removed
#define APK_DIGEST_SHA1		0x02
#define APK_DIGEST_SHA256	0x03
#define APK_DIGEST_SHA512	0x04
#define APK_DIGEST_SHA256_160	0x05

#define APK_DIGEST_LENGTH_SHA1		20
#define APK_DIGEST_LENGTH_SHA256_160	20
#define APK_DIGEST_LENGTH_SHA256	32
#define APK_DIGEST_LENGTH_SHA512	64
#define APK_DIGEST_LENGTH_MAX		APK_DIGEST_LENGTH_SHA512

/* Enough space for a hexdump of the longest checksum possible plus
 * a two-character type prefix */
#define APK_BLOB_DIGEST_BUF	(2 + (2 * APK_DIGEST_LENGTH_MAX))

const char *apk_digest_alg_str(uint8_t);
uint8_t apk_digest_alg_by_str(const char *algstr);

struct apk_digest {
	uint8_t alg, len;
	uint8_t data[APK_DIGEST_LENGTH_MAX];
};

#define APK_DIGEST_BLOB(d) APK_BLOB_PTR_LEN((void*)((d).data), (d).len)

int apk_digest_alg_len(uint8_t alg);
uint8_t apk_digest_alg_by_len(int len);
uint8_t apk_digest_from_blob(struct apk_digest *d, apk_blob_t b);

int apk_digest_calc(struct apk_digest *d, uint8_t alg, const void *ptr, size_t sz);

static inline int apk_digest_cmp(struct apk_digest *a, struct apk_digest *b) {
	if (a->alg != b->alg) return b->alg - a->alg;
	return memcmp(a->data, b->data, a->len);
}

static inline void apk_digest_reset(struct apk_digest *d) {
	d->alg = APK_DIGEST_NONE;
	d->len = 0;
}

static inline void apk_digest_set(struct apk_digest *d, uint8_t alg) {
	d->alg = alg;
	d->len = apk_digest_alg_len(alg);
}

static inline int apk_digest_cmp_blob(const struct apk_digest *d, uint8_t alg, const apk_blob_t b) {
	if (d->alg != alg) return (int)alg - (int)d->alg;
	return apk_blob_compare(APK_DIGEST_BLOB(*d), b);
}

static inline void apk_digest_push(apk_blob_t *to, struct apk_digest *digest) {
	return apk_blob_push_hash(to, APK_DIGEST_BLOB(*digest));
}

static inline void apk_digest_push_hex(apk_blob_t *to, struct apk_digest *digest) {
	return apk_blob_push_hash_hex(to, APK_DIGEST_BLOB(*digest));
}

// Digest context

struct apk_digest_ctx {
	uint8_t alg;
	void *priv;
};

int apk_digest_ctx_init(struct apk_digest_ctx *dctx, uint8_t alg);
int apk_digest_ctx_reset(struct apk_digest_ctx *dctx);
int apk_digest_ctx_reset_alg(struct apk_digest_ctx *dctx, uint8_t alg);
void apk_digest_ctx_free(struct apk_digest_ctx *dctx);
int apk_digest_ctx_update(struct apk_digest_ctx *dctx, const void *ptr, size_t sz);
int apk_digest_ctx_final(struct apk_digest_ctx *dctx, struct apk_digest *d);

// Asymmetric keys

struct apk_pkey {
	uint8_t id[16];
	void *priv;
};

void apk_pkey_free(struct apk_pkey *pkey);
int apk_pkey_load(struct apk_pkey *pkey, int dirfd, const char *fn, int priv);

// Signing

int apk_sign_start(struct apk_digest_ctx *, uint8_t, struct apk_pkey *);
int apk_sign(struct apk_digest_ctx *, void *, size_t *);
int apk_verify_start(struct apk_digest_ctx *, uint8_t, struct apk_pkey *);
int apk_verify(struct apk_digest_ctx *, void *, size_t);

// Initializiation

void apk_crypto_init(void);
