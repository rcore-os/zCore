#include <errno.h>
#include <stdio.h>
#include <fcntl.h>
#include <openssl/bio.h>
#include <openssl/pem.h>
#include <openssl/err.h>
#include <openssl/opensslv.h>
#include <openssl/crypto.h>
#include <openssl/evp.h>
#ifndef OPENSSL_NO_ENGINE
#include <openssl/engine.h>
#endif

#include "apk_crypto.h"

// Copmatibility with older openssl

#if OPENSSL_VERSION_NUMBER < 0x1010000fL || (defined(LIBRESSL_VERSION_NUMBER) && LIBRESSL_VERSION_NUMBER < 0x2070000fL)

static inline EVP_MD_CTX *EVP_MD_CTX_new(void)
{
	return EVP_MD_CTX_create();
}

static inline void EVP_MD_CTX_free(EVP_MD_CTX *mdctx)
{
	return EVP_MD_CTX_destroy(mdctx);
}

#endif

// OpenSSL opaque types mapped directly to the priv

static EVP_MD_CTX *ossl_mdctx(struct apk_digest_ctx *dctx) { return dctx->priv; }
static void apk_digest_set_mdctx(struct apk_digest_ctx *dctx, EVP_MD_CTX *mdctx)
{
	EVP_MD_CTX_free(dctx->priv);
	dctx->priv = mdctx;
}

static EVP_PKEY *ossl_pkey(struct apk_pkey *pkey) { return pkey->priv; }
static void apk_pkey_set_pkey(struct apk_pkey *pkey, EVP_PKEY *pk)
{
	EVP_PKEY_free(pkey->priv);
	pkey->priv = pk;
}

#if OPENSSL_VERSION_NUMBER >= 0x30000000L
static EVP_MD *sha1 = NULL;
static EVP_MD *sha256 = NULL;
static EVP_MD *sha512 = NULL;

static inline void lookup_algorithms(void)
{
	sha1 = EVP_MD_fetch(NULL, "sha1", NULL);
	sha256 = EVP_MD_fetch(NULL, "sha256", NULL);
	sha512 = EVP_MD_fetch(NULL, "sha512", NULL);
}

static inline void free_algorithms(void)
{
	EVP_MD_free(sha1);
	EVP_MD_free(sha256);
	EVP_MD_free(sha512);
}
#else
static const EVP_MD *sha1 = NULL;
static const EVP_MD *sha256 = NULL;
static const EVP_MD *sha512 = NULL;

static inline void lookup_algorithms(void)
{
	sha1 = EVP_sha1();
	sha256 = EVP_sha256();
	sha512 = EVP_sha512();
}

static inline void free_algorithms(void)
{
}
#endif

static inline const EVP_MD *apk_digest_alg_to_evp(uint8_t alg) {
	/*
	 * "none"/EVP_md_null is broken on several versions of libcrypto and should be avoided.
	 */
	switch (alg) {
	case APK_DIGEST_NONE:	return NULL;
	case APK_DIGEST_SHA1:	return sha1;
	case APK_DIGEST_SHA256_160:
	case APK_DIGEST_SHA256:	return sha256;
	case APK_DIGEST_SHA512:	return sha512;
	default:
		assert(!"valid alg");
		return NULL;
	}
}

int apk_digest_calc(struct apk_digest *d, uint8_t alg, const void *ptr, size_t sz)
{
	unsigned int md_sz = sizeof d->data;
	if (EVP_Digest(ptr, sz, d->data, &md_sz, apk_digest_alg_to_evp(alg), 0) != 1)
		return -APKE_CRYPTO_ERROR;
	apk_digest_set(d, alg);
	return 0;
}

int apk_digest_ctx_init(struct apk_digest_ctx *dctx, uint8_t alg)
{
	dctx->alg = alg;
	dctx->priv = NULL;

	apk_digest_set_mdctx(dctx, EVP_MD_CTX_new());
	if (!ossl_mdctx(dctx)) return -ENOMEM;
#ifdef EVP_MD_CTX_FLAG_FINALISE
	EVP_MD_CTX_set_flags(ossl_mdctx(dctx), EVP_MD_CTX_FLAG_FINALISE);
#endif
	if (dctx->alg == APK_DIGEST_NONE) return 0;
	if (EVP_DigestInit_ex(ossl_mdctx(dctx), apk_digest_alg_to_evp(alg), 0) != 1)
		return -APKE_CRYPTO_ERROR;
	return 0;
}

int apk_digest_ctx_reset(struct apk_digest_ctx *dctx)
{
	if (dctx->alg == APK_DIGEST_NONE) return 0;
	if (EVP_DigestInit_ex(ossl_mdctx(dctx), NULL, 0) != 1) return -APKE_CRYPTO_ERROR;
	return 0;
}

int apk_digest_ctx_reset_alg(struct apk_digest_ctx *dctx, uint8_t alg)
{
	assert(alg != APK_DIGEST_NONE);
	if (EVP_MD_CTX_reset(ossl_mdctx(dctx)) != 1 ||
	    EVP_DigestInit_ex(ossl_mdctx(dctx), apk_digest_alg_to_evp(alg), 0) != 1)
		return -APKE_CRYPTO_ERROR;
	dctx->alg = alg;
	return 0;
}

void apk_digest_ctx_free(struct apk_digest_ctx *dctx)
{
	apk_digest_set_mdctx(dctx, NULL);
}

int apk_digest_ctx_update(struct apk_digest_ctx *dctx, const void *ptr, size_t sz)
{
	assert(dctx->alg != APK_DIGEST_NONE);
	return EVP_DigestUpdate(ossl_mdctx(dctx), ptr, sz) == 1 ? 0 : -APKE_CRYPTO_ERROR;
}

int apk_digest_ctx_final(struct apk_digest_ctx *dctx, struct apk_digest *d)
{
	unsigned int mdlen = sizeof d->data;

	assert(dctx->alg != APK_DIGEST_NONE);

	if (EVP_DigestFinal_ex(ossl_mdctx(dctx), d->data, &mdlen) != 1) {
		apk_digest_reset(d);
		return -APKE_CRYPTO_ERROR;
	}
	d->alg = dctx->alg;
	d->len = apk_digest_alg_len(d->alg);
	return 0;
}

static int apk_pkey_init(struct apk_pkey *pkey, EVP_PKEY *key)
{
	unsigned char dig[EVP_MAX_MD_SIZE], *pub = NULL;
	unsigned int dlen = sizeof dig;
	int len, r = -APKE_CRYPTO_ERROR;

	pkey->priv = NULL;
	if ((len = i2d_PublicKey(key, &pub)) < 0) return -APKE_CRYPTO_ERROR;
	if (EVP_Digest(pub, len, dig, &dlen, EVP_sha512(), NULL) == 1) {
		memcpy(pkey->id, dig, sizeof pkey->id);
		r = 0;
	}
	OPENSSL_free(pub);
	apk_pkey_set_pkey(pkey, key);

	return r;
}

void apk_pkey_free(struct apk_pkey *pkey)
{
	apk_pkey_set_pkey(pkey, NULL);
}

int apk_pkey_load(struct apk_pkey *pkey, int dirfd, const char *fn, int priv)
{
	EVP_PKEY *key;
	BIO *bio;
	int fd;

	fd = openat(dirfd, fn, O_RDONLY | O_CLOEXEC);
	if (fd < 0) return -errno;

	bio = BIO_new_fp(fdopen(fd, "r"), BIO_CLOSE);
	if (!bio) return -ENOMEM;
	if (priv)
		key = PEM_read_bio_PrivateKey(bio, NULL, NULL, NULL);
	else
		key = PEM_read_bio_PUBKEY(bio, NULL, NULL, NULL);
	BIO_free(bio);
	if (!key) return -APKE_CRYPTO_KEY_FORMAT;

	return apk_pkey_init(pkey, key);
}

int apk_sign_start(struct apk_digest_ctx *dctx, uint8_t alg, struct apk_pkey *pkey)
{
	if (EVP_MD_CTX_reset(ossl_mdctx(dctx)) != 1 ||
	    EVP_DigestSignInit(ossl_mdctx(dctx), NULL, apk_digest_alg_to_evp(alg), NULL, ossl_pkey(pkey)) != 1)
		return -APKE_CRYPTO_ERROR;
	dctx->alg = alg;
	return 0;
}

int apk_sign(struct apk_digest_ctx *dctx, void *sig, size_t *len)
{
	if (EVP_DigestSignFinal(ossl_mdctx(dctx), sig, len) != 1)
		return -APKE_SIGNATURE_GEN_FAILURE;
	return 0;
}

int apk_verify_start(struct apk_digest_ctx *dctx, uint8_t alg, struct apk_pkey *pkey)
{
	if (EVP_MD_CTX_reset(ossl_mdctx(dctx)) != 1 ||
	    EVP_DigestVerifyInit(ossl_mdctx(dctx), NULL, apk_digest_alg_to_evp(alg), NULL, ossl_pkey(pkey)) != 1)
		return -APKE_CRYPTO_ERROR;
	dctx->alg = alg;
	return 0;
}

int apk_verify(struct apk_digest_ctx *dctx, void *sig, size_t len)
{
	if (EVP_DigestVerifyFinal(ossl_mdctx(dctx), sig, len) != 1)
		return -APKE_SIGNATURE_INVALID;
	return 0;
}

static void apk_crypto_cleanup(void)
{
	free_algorithms();

#if OPENSSL_VERSION_NUMBER < 0x1010000fL || (defined(LIBRESSL_VERSION_NUMBER) && LIBRESSL_VERSION_NUMBER < 0x2070000fL)
	EVP_cleanup();
#ifndef OPENSSL_NO_ENGINE
	ENGINE_cleanup();
#endif
	CRYPTO_cleanup_all_ex_data();
#endif
}

void apk_crypto_init(void)
{
	atexit(apk_crypto_cleanup);

#if OPENSSL_VERSION_NUMBER < 0x1010000fL || (defined(LIBRESSL_VERSION_NUMBER) && LIBRESSL_VERSION_NUMBER < 0x2070000fL)
	OpenSSL_add_all_algorithms();
#ifndef OPENSSL_NO_ENGINE
	ENGINE_load_builtin_engines();
	ENGINE_register_all_complete();
#endif
#endif

	lookup_algorithms();
}
