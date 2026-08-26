/* crypto_mbedtls.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2024 Jonas Jelonek <jelonek.jonas@gmail.com>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <fcntl.h>
#include <sys/random.h>
#include <sys/stat.h>
#include <unistd.h>

#include <mbedtls/platform.h>
#include <mbedtls/bignum.h>
#include <mbedtls/md.h>
#include <mbedtls/pk.h>
#include <mbedtls/entropy.h>

#ifdef MBEDTLS_PSA_CRYPTO_C
#include <psa/crypto.h>
#endif

#include "apk_crypto.h"

struct apk_mbed_digest {
	struct apk_pkey *sigver_key;
	mbedtls_md_context_t md;
};

struct apk_mbed_pkey {
	mbedtls_pk_context pk;
};

static struct apk_mbed_digest *mbed_digest(struct apk_digest_ctx *dctx) { return dctx->priv; };
static struct apk_mbed_pkey *mbed_pkey(struct apk_pkey *pkey) { return pkey->priv; };

/* based on mbedtls' internal pkwrite.h calculations */
#define APK_ENC_KEY_MAX_LENGTH          (38 + 2 * MBEDTLS_MPI_MAX_SIZE)
/* sane limit for keyfiles with PEM, long keys and maybe comments */
#define APK_KEYFILE_MAX_LENGTH		64000

static inline const mbedtls_md_type_t apk_digest_alg_to_mbedtls_type(uint8_t alg) {
	switch (alg) {
	case APK_DIGEST_NONE:	return MBEDTLS_MD_NONE;
	case APK_DIGEST_SHA1:	return MBEDTLS_MD_SHA1;
	case APK_DIGEST_SHA256_160:
	case APK_DIGEST_SHA256:	return MBEDTLS_MD_SHA256;
	case APK_DIGEST_SHA512:	return MBEDTLS_MD_SHA512;
	default:
		assert(!"valid alg");
		return MBEDTLS_MD_NONE;
	}
}

static inline const mbedtls_md_info_t *apk_digest_alg_to_mdinfo(uint8_t alg)
{
	return mbedtls_md_info_from_type(
		apk_digest_alg_to_mbedtls_type(alg)
	);
}

int apk_digest_calc(struct apk_digest *d, uint8_t alg, const void *ptr, size_t sz)
{
	if (mbedtls_md(apk_digest_alg_to_mdinfo(alg), ptr, sz, d->data))
		return -APKE_CRYPTO_ERROR;

	apk_digest_set(d, alg);
	return 0;
}

int apk_digest_ctx_init(struct apk_digest_ctx *dctx, uint8_t alg)
{
	struct apk_mbed_digest *md;

	dctx->alg = alg;
	dctx->priv = md = calloc(1, sizeof *md);
	if (!dctx->priv) return -ENOMEM;

	mbedtls_md_init(&md->md);
	if (alg == APK_DIGEST_NONE) return 0;
	if (mbedtls_md_setup(&md->md, apk_digest_alg_to_mdinfo(alg), 0) ||
		mbedtls_md_starts(&md->md))
		return -APKE_CRYPTO_ERROR;

	return 0;
}

int apk_digest_ctx_reset(struct apk_digest_ctx *dctx)
{
	struct apk_mbed_digest *md = mbed_digest(dctx);

	if (dctx->alg == APK_DIGEST_NONE) return 0;
	if (mbedtls_md_starts(&md->md)) return -APKE_CRYPTO_ERROR;
	return 0;
}

int apk_digest_ctx_reset_alg(struct apk_digest_ctx *dctx, uint8_t alg)
{
	struct apk_mbed_digest *md = mbed_digest(dctx);

	assert(alg != APK_DIGEST_NONE);

	mbedtls_md_free(&md->md);
	dctx->alg = alg;
	md->sigver_key = NULL;
	if (mbedtls_md_setup(&md->md, apk_digest_alg_to_mdinfo(alg), 0) ||
	    mbedtls_md_starts(&md->md))
		return -APKE_CRYPTO_ERROR;

	return 0;
}

void apk_digest_ctx_free(struct apk_digest_ctx *dctx)
{
	struct apk_mbed_digest *md = mbed_digest(dctx);

	if (md != NULL) {
		mbedtls_md_free(&md->md);
		free(md);
		dctx->priv = NULL;
	}
}

int apk_digest_ctx_update(struct apk_digest_ctx *dctx, const void *ptr, size_t sz)
{
	struct apk_mbed_digest *md = mbed_digest(dctx);

	assert(dctx->alg != APK_DIGEST_NONE);
	return mbedtls_md_update(&md->md, ptr, sz) == 0 ? 0 : -APKE_CRYPTO_ERROR;
}

int apk_digest_ctx_final(struct apk_digest_ctx *dctx, struct apk_digest *d)
{
	struct apk_mbed_digest *md = mbed_digest(dctx);

	assert(dctx->alg != APK_DIGEST_NONE);
	if (mbedtls_md_finish(&md->md, d->data)) {
		apk_digest_reset(d);
		return -APKE_CRYPTO_ERROR;
	}
	d->alg = dctx->alg;
	d->len = apk_digest_alg_len(d->alg);
	return 0;
}

static int apk_load_file_at(int dirfd, const char *fn, unsigned char **buf, size_t *n)
{
	struct stat stats;
	size_t size;
	int fd;

	if ((fd = openat(dirfd, fn, O_RDONLY | O_CLOEXEC)) < 0)
		return -errno;

	if (fstat(fd, &stats)) {
		close(fd);
		return -errno;
	}

	size = (size_t)stats.st_size;
	*n = size;

	if (!size || size > APK_KEYFILE_MAX_LENGTH)
		return -APKE_CRYPTO_KEY_FORMAT;
	if ((*buf = mbedtls_calloc(1, size + 1)) == NULL)
		return -ENOMEM;

	if (read(fd, *buf, size) != size) {
		int ret = -errno;
		close(fd);
		mbedtls_platform_zeroize(*buf, size);
		mbedtls_free(*buf);
		return ret;
	}
	close(fd);

	(*buf)[size] = '\0';

	/* if it's a PEM key increment length since mbedtls requires
	 * buffer to be null-terminated for PEM */
	if (strstr((const char *) *buf, "-----BEGIN ") != NULL)
		++*n;

	return 0;
}

static int apk_pkey_fingerprint(struct apk_pkey *pkey)
{
	struct apk_mbed_pkey *mp = mbed_pkey(pkey);
	unsigned char dig[APK_DIGEST_LENGTH_MAX];
	unsigned char pub[APK_ENC_KEY_MAX_LENGTH] = {};
	unsigned char *c;
	int len, r = -APKE_CRYPTO_ERROR;

	c = pub + APK_ENC_KEY_MAX_LENGTH;

	// key is written backwards into pub starting at c!
	if ((len = mbedtls_pk_write_pubkey(&c, pub, &mp->pk)) < 0) return -APKE_CRYPTO_ERROR;
	if (!mbedtls_md(apk_digest_alg_to_mdinfo(APK_DIGEST_SHA512), c, len, dig)) {
		memcpy(pkey->id, dig, sizeof pkey->id);
		r = 0;
	}

	return r;
}

void apk_pkey_free(struct apk_pkey *pkey)
{
	struct apk_mbed_pkey *mp = mbed_pkey(pkey);

	if (mp) {
		mbedtls_pk_free(&mp->pk);
		free(mp);
		pkey->priv = NULL;
	}
}

static int apk_mbedtls_random(void *ctx, unsigned char *out, size_t len)
{
	return (int)getrandom(out, len, 0);
}

#if MBEDTLS_VERSION_NUMBER >= 0x03000000
static inline int apk_mbedtls_parse_privkey(struct apk_pkey *pkey, const unsigned char *buf, size_t blen)
{
	return mbedtls_pk_parse_key(&mbed_pkey(pkey)->pk, buf, blen, NULL, 0, apk_mbedtls_random, NULL);
}
static inline int apk_mbedtls_sign(struct apk_digest_ctx *dctx, struct apk_digest *dig,
				   unsigned char *sig, size_t *sig_len)
{
	struct apk_mbed_digest *md = mbed_digest(dctx);
	struct apk_mbed_pkey *mp = mbed_pkey(md->sigver_key);

	return mbedtls_pk_sign(&mp->pk, apk_digest_alg_to_mbedtls_type(dctx->alg),
			       (const unsigned char *)&dig->data, dig->len, sig, *sig_len, sig_len,
			       apk_mbedtls_random, NULL);
}
#else
static inline int apk_mbedtls_parse_privkey(struct apk_pkey *pkey, const unsigned char *buf, size_t blen)
{
	return mbedtls_pk_parse_key(&mbed_pkey(pkey)->pk, buf, blen, NULL, 0);
}
static inline int apk_mbedtls_sign(struct apk_digest_ctx *dctx, struct apk_digest *dig,
				   unsigned char *sig, size_t *sig_len)
{
	struct apk_mbed_digest *md = mbed_digest(dctx);
	struct apk_mbed_pkey *mp = mbed_pkey(md->sigver_key);

	return mbedtls_pk_sign(&mp->pkg, apk_digest_alg_to_mbedtls_type(dctx->alg),
			       (const unsigned char *)&dig->data, dig->len, sig, sig_len,
			       apk_mbedtls_random, NULL);
}
#endif

int apk_pkey_load(struct apk_pkey *pkey, int dirfd, const char *fn, int priv)
{
	struct apk_mbed_pkey *mp = NULL;
	unsigned char *buf = NULL;
	size_t blen = 0;
	int ret;

	pkey->priv = NULL;
	mp = calloc(1, sizeof *mp);
	if (!mp) return -ENOMEM;

	mbedtls_pk_init(&mp->pk);
	pkey->priv = mp;

	ret = apk_load_file_at(dirfd, fn, &buf, &blen);
	if (ret) {
		apk_pkey_free(pkey);
		return ret;
	}

	if (priv)
		ret = apk_mbedtls_parse_privkey(pkey, buf, blen);
	else
		ret = mbedtls_pk_parse_public_key(&mp->pk, buf, blen);

	mbedtls_platform_zeroize(buf, blen);
	mbedtls_free(buf);

	if (ret == 0) ret = apk_pkey_fingerprint(pkey);
	if (ret != 0) {
		apk_pkey_free(pkey);
		return -APKE_CRYPTO_ERROR;
	}

	return ret;
}

int apk_sign_start(struct apk_digest_ctx *dctx, uint8_t alg, struct apk_pkey *pkey)
{
	struct apk_mbed_digest *md = mbed_digest(dctx);

	if (apk_digest_ctx_reset_alg(dctx, alg))
		return -APKE_CRYPTO_ERROR;

	md->sigver_key = pkey;
	return 0;
}

int apk_sign(struct apk_digest_ctx *dctx, void *sig, size_t *len)
{
	struct apk_mbed_digest *md = mbed_digest(dctx);
	struct apk_digest dig;
	int r = 0;

	if (!md->sigver_key)
		return -APKE_CRYPTO_ERROR;

	if (apk_digest_ctx_final(dctx, &dig) || apk_mbedtls_sign(dctx, &dig, sig, len))
		r = -APKE_SIGNATURE_GEN_FAILURE;

	md->sigver_key = NULL;
	return r;
}

int apk_verify_start(struct apk_digest_ctx *dctx, uint8_t alg, struct apk_pkey *pkey)
{
	struct apk_mbed_digest *md = mbed_digest(dctx);

	if (apk_digest_ctx_reset_alg(dctx, alg))
		return -APKE_CRYPTO_ERROR;

	md->sigver_key = pkey;
	return 0;
}

int apk_verify(struct apk_digest_ctx *dctx, void *sig, size_t len)
{
	struct apk_mbed_digest *md = mbed_digest(dctx);
	struct apk_digest dig;
	int r = 0;

	if (!md->sigver_key)
		return -APKE_CRYPTO_ERROR;

	if (apk_digest_ctx_final(dctx, &dig)) {
		r = -APKE_CRYPTO_ERROR;
		goto final;
	}
	if (mbedtls_pk_verify(&mbed_pkey(md->sigver_key)->pk,
			      apk_digest_alg_to_mbedtls_type(dctx->alg),
			      (const unsigned char *)&dig.data, dig.len, sig, len))
		r = -APKE_SIGNATURE_INVALID;

final:
	md->sigver_key = NULL;
	return r;
}

static void apk_crypto_cleanup(void)
{
#ifdef MBEDTLS_PSA_CRYPTO_C
	mbedtls_psa_crypto_free();
#endif
}

void apk_crypto_init(void)
{
	atexit(apk_crypto_cleanup);
	
#ifdef MBEDTLS_PSA_CRYPTO_C
	psa_crypto_init();
#endif
}
