/* extract_v2.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include "apk_context.h"
#include "apk_extract.h"
#include "apk_package.h"
#include "apk_crypto.h"
#include "apk_tar.h"

#define APK_SIGN_VERIFY			1
#define APK_SIGN_VERIFY_IDENTITY	2
#define APK_SIGN_VERIFY_AND_GENERATE	3

struct apk_sign_ctx {
	struct apk_extract_ctx *ectx;
	struct apk_trust *trust;
	int action;
	int num_signatures;
	int verify_error;
	unsigned char control_started : 1;
	unsigned char data_started : 1;
	unsigned char has_data_checksum : 1;
	unsigned char control_verified : 1;
	unsigned char data_verified : 1;
	unsigned char allow_untrusted : 1;
	unsigned char end_seen : 1;
	uint8_t alg;
	struct apk_digest data_hash;
	struct apk_digest_ctx digest_ctx;
	struct apk_digest_ctx identity_ctx;

	struct {
		apk_blob_t data;
		struct apk_pkey *pkey;
		char *identity;
	} signature;
};

static void apk_sign_ctx_init(struct apk_sign_ctx *ctx, int action, struct apk_extract_ctx *ectx, struct apk_trust *trust)
{
	memset(ctx, 0, sizeof(struct apk_sign_ctx));
	ctx->trust = trust;
	ctx->action = action;
	ctx->allow_untrusted = trust->allow_untrusted;
	ctx->verify_error = -APKE_SIGNATURE_UNTRUSTED;
	ctx->alg = APK_DIGEST_SHA1;
	ctx->ectx = ectx;
	switch (action) {
	case APK_SIGN_VERIFY_AND_GENERATE:
		apk_digest_ctx_init(&ctx->identity_ctx, APK_DIGEST_SHA1);
		break;
	case APK_SIGN_VERIFY:
	case APK_SIGN_VERIFY_IDENTITY:
		break;
	default:
		assert(!"unreachable");
		break;
	}
	apk_digest_ctx_init(&ctx->digest_ctx, ctx->alg);
}

static void apk_sign_ctx_free(struct apk_sign_ctx *ctx)
{
	free(ctx->signature.data.ptr);
	apk_digest_ctx_free(&ctx->identity_ctx);
	apk_digest_ctx_free(&ctx->digest_ctx);
}

static int check_signing_key_trust(struct apk_sign_ctx *sctx)
{
	switch (sctx->action) {
	case APK_SIGN_VERIFY:
	case APK_SIGN_VERIFY_AND_GENERATE:
		if (sctx->signature.pkey == NULL) {
			if (sctx->allow_untrusted)
				break;
			return -APKE_SIGNATURE_UNTRUSTED;
		}
	}
	return 0;
}

static int apk_sign_ctx_process_file(struct apk_sign_ctx *ctx, const struct apk_file_info *fi,
		struct apk_istream *is)
{
	static struct {
		char type[7];
		uint8_t alg;
	} signature_type[] = {
		{ "RSA512", APK_DIGEST_SHA512 },
		{ "RSA256", APK_DIGEST_SHA256 },
		{ "RSA", APK_DIGEST_SHA1 },
		{ "DSA", APK_DIGEST_SHA1 },
	};
	uint8_t alg = APK_DIGEST_NONE;
	const char *name = NULL;
	struct apk_pkey *pkey;
	int r, i;

	if (ctx->data_started)
		return 1;

	if (fi->name[0] != '.' || strchr(fi->name, '/') != NULL) {
		/* APKv1.0 compatibility - first non-hidden file is
		 * considered to start the data section of the file.
		 * This does not make any sense if the file has v2.0
		 * style .PKGINFO */
		if (ctx->has_data_checksum)
			return -APKE_V2PKG_FORMAT;
		/* Error out early if identity part is missing */
		if (ctx->action == APK_SIGN_VERIFY_IDENTITY)
			return -APKE_V2PKG_FORMAT;
		ctx->data_started = 1;
		ctx->control_started = 1;
		r = check_signing_key_trust(ctx);
		if (r != 0) return r;
		return 1;
	}

	if (ctx->control_started)
		return 1;

	if (strncmp(fi->name, ".SIGN.", 6) != 0) {
		ctx->control_started = 1;
		return 1;
	}

	/* By this point, we must be handling a signature file */
	ctx->num_signatures++;

	/* Already found a signature by a trusted key; no need to keep searching */
	if (ctx->signature.pkey != NULL) return 0;
	if (ctx->action == APK_SIGN_VERIFY_IDENTITY) return 0;

	for (i = 0; i < ARRAY_SIZE(signature_type); i++) {
		size_t slen = strlen(signature_type[i].type);
		if (strncmp(&fi->name[6], signature_type[i].type, slen) == 0 &&
		    fi->name[6+slen] == '.') {
			alg = signature_type[i].alg;
			name = &fi->name[6+slen+1];
			break;
		}
	}
	if (alg == APK_DIGEST_NONE) return 0;
	if (fi->size > 65536) return 0;

	pkey = apk_trust_key_by_name(ctx->trust, name);
	if (pkey) {
		ctx->alg = alg;
		ctx->signature.pkey = pkey;
		apk_blob_from_istream(is, fi->size, &ctx->signature.data);
	}
	return 0;
}


/*	apk_sign_ctx_mpart_cb() handles hashing archives and checking signatures, but
	it can't do it alone. apk_sign_ctx_process_file() must be in the loop to
	actually select which signature is to be verified and load the corresponding
	public key into the context object, and	apk_sign_ctx_parse_pkginfo_line()
	needs to be called when handling the .PKGINFO file to find any applicable
	datahash and load it into the context for this function to check against. */
static int apk_sign_ctx_mpart_cb(void *ctx, int part, apk_blob_t data)
{
	struct apk_sign_ctx *sctx = (struct apk_sign_ctx *) ctx;
	struct apk_digest calculated;
	int r, end_of_control;

	if (sctx->end_seen || sctx->data_verified) return -APKE_FORMAT_INVALID;
	if (part == APK_MPART_BOUNDARY && sctx->data_started) return -APKE_FORMAT_INVALID;
	if (part == APK_MPART_END) sctx->end_seen = 1;
	if (part == APK_MPART_DATA) {
		/* Update digest with the data now. Only _DATA callbacks can have data. */
		r = apk_digest_ctx_update(&sctx->digest_ctx, data.ptr, data.len);
		if (r != 0) return r;

		/* Update identity generated also if needed. */
		if (sctx->control_started && !sctx->data_started &&
		    sctx->identity_ctx.alg != APK_DIGEST_NONE) {
			r = apk_digest_ctx_update(&sctx->identity_ctx, data.ptr, data.len);
			if (r != 0) return r;
		}
		return 0;
	}
	if (data.len) return -APKE_FORMAT_INVALID;

	/* Still in signature blocks? */
	if (!sctx->control_started) {
		if (part == APK_MPART_END) return -APKE_FORMAT_INVALID;

		r = apk_digest_ctx_reset(&sctx->identity_ctx);
		if (r != 0) return r;

		/* Control block starting, prepare for signature verification */
		if (sctx->signature.pkey == NULL || sctx->action == APK_SIGN_VERIFY_IDENTITY)
			return apk_digest_ctx_reset_alg(&sctx->digest_ctx, sctx->alg);

		return apk_verify_start(&sctx->digest_ctx, sctx->alg, sctx->signature.pkey);
	}

	/* Grab state and mark all remaining block as data */
	end_of_control = (sctx->data_started == 0);
	sctx->data_started = 1;

	/* End of control-block and control does not have data checksum? */
	if (sctx->has_data_checksum == 0 && end_of_control && part != APK_MPART_END)
		return 0;

	if (sctx->has_data_checksum && !end_of_control) {
		/* End of data-block with a checksum read from the control block */
		r = apk_digest_ctx_final(&sctx->digest_ctx, &calculated);
		if (r != 0) return r;
		if (apk_digest_cmp(&calculated, &sctx->data_hash) != 0)
			return -APKE_V2PKG_INTEGRITY;
		sctx->data_verified = 1;
		if (!sctx->allow_untrusted && !sctx->control_verified)
			return -APKE_SIGNATURE_UNTRUSTED;
		return 0;
	}

	/* Either end of control block with a data checksum or end
	 * of the data block following a control block without a data
	 * checksum. In either case, we're checking a signature. */
	r = check_signing_key_trust(sctx);
	if (r != 0) return r;

	switch (sctx->action) {
	case APK_SIGN_VERIFY_AND_GENERATE:
		/* Package identity is the checksum */
		apk_digest_ctx_final(&sctx->identity_ctx, sctx->ectx->generate_identity);
		if (!sctx->has_data_checksum) return -APKE_V2PKG_FORMAT;
		/* Fallthrough to check signature */
	case APK_SIGN_VERIFY:
		if (sctx->signature.pkey != NULL) {
			sctx->verify_error = apk_verify(&sctx->digest_ctx,
				(unsigned char *) sctx->signature.data.ptr,
				sctx->signature.data.len);
		}
		if (sctx->verify_error) {
			if (sctx->verify_error != -APKE_SIGNATURE_UNTRUSTED ||
			    !sctx->allow_untrusted)
				return sctx->verify_error;
		}
		sctx->control_verified = 1;
		if (!sctx->has_data_checksum && part == APK_MPART_END)
			sctx->data_verified = 1;
		break;
	case APK_SIGN_VERIFY_IDENTITY:
		/* Reset digest for hashing data */
		apk_digest_ctx_final(&sctx->digest_ctx, &calculated);
		if (apk_digest_cmp_blob(&calculated, sctx->ectx->verify_alg, sctx->ectx->verify_digest) != 0)
			return -APKE_V2PKG_INTEGRITY;
		sctx->verify_error = 0;
		sctx->control_verified = 1;
		if (!sctx->has_data_checksum && part == APK_MPART_END)
			sctx->data_verified = 1;
		break;
	}

	r = apk_digest_ctx_reset(&sctx->identity_ctx);
	if (r != 0) return r;

	return apk_digest_ctx_reset_alg(&sctx->digest_ctx, sctx->alg);
}

static int apk_extract_verify_v2index(struct apk_extract_ctx *ectx, apk_blob_t *desc, struct apk_istream *is)
{
	return 0;
}

static int apk_extract_verify_v2file(struct apk_extract_ctx *ectx, const struct apk_file_info *fi, struct apk_istream *is)
{
	return 0;
}

static const struct apk_extract_ops extract_v2verify_ops = {
	.v2index = apk_extract_verify_v2index,
	.v2meta = apk_extract_v2_meta,
	.file = apk_extract_verify_v2file,
};

static int apk_extract_v2_entry(void *pctx, const struct apk_file_info *fi, struct apk_istream *is)
{
	struct apk_extract_ctx *ectx = pctx;
	struct apk_sign_ctx *sctx = ectx->pctx;
	int r, type;

	r = apk_sign_ctx_process_file(sctx, fi, is);
	if (r <= 0) return r;

	if (!sctx->control_started) return 0;
	if (!sctx->data_started || !sctx->has_data_checksum) {
		if (fi->name[0] == '.') {
			ectx->is_package = 1;
			if (ectx->is_index) return -APKE_V2NDX_FORMAT;
			if (!ectx->ops->v2meta) return -APKE_FORMAT_NOT_SUPPORTED;
			if (strcmp(fi->name, ".PKGINFO") == 0) {
				return ectx->ops->v2meta(ectx, is);
			} else if (strcmp(fi->name, ".INSTALL") == 0) {
				return -APKE_V2PKG_FORMAT;
			} else if ((type = apk_script_type(&fi->name[1])) != APK_SCRIPT_INVALID) {
				if (ectx->ops->script) return ectx->ops->script(ectx, type, fi->size, is);
			}
		} else {
			ectx->is_index = 1;
			if (ectx->is_package) return -APKE_V2PKG_FORMAT;
			if (!ectx->ops->v2index) return -APKE_FORMAT_NOT_SUPPORTED;
			if (strcmp(fi->name, "DESCRIPTION") == 0 && fi->size <= 160) {
				free(ectx->desc.ptr);
				apk_blob_from_istream(is, fi->size, &ectx->desc);
			} else if (strcmp(fi->name, "APKINDEX") == 0) {
				return ectx->ops->v2index(ectx, &ectx->desc, is);
			}
		}
		return 0;
	}

	if (!sctx->data_started) return 0;
	if (!ectx->ops->file) return -ECANCELED;
	if (fi->name[0] == '.') return 0;
	return ectx->ops->file(ectx, fi, is);
}

int apk_extract_v2(struct apk_extract_ctx *ectx, struct apk_istream *is)
{
	struct apk_ctx *ac = ectx->ac;
	struct apk_trust *trust = apk_ctx_get_trust(ac);
	struct apk_sign_ctx sctx;
	int r, action;

	if (ectx->generate_identity)
		action = APK_SIGN_VERIFY_AND_GENERATE;
	else if (ectx->verify_alg != APK_DIGEST_NONE)
		action = APK_SIGN_VERIFY_IDENTITY;
	else
		action = APK_SIGN_VERIFY;

	if (!ectx->ops) ectx->ops = &extract_v2verify_ops;
	ectx->pctx = &sctx;
	apk_sign_ctx_init(&sctx, action, ectx, trust);
	r = apk_tar_parse(
		apk_istream_gunzip_mpart(is, apk_sign_ctx_mpart_cb, &sctx),
		apk_extract_v2_entry, ectx, apk_ctx_get_id_cache(ac));
	if ((r == 0 || r == -ECANCELED || r == -APKE_EOF) && !ectx->is_package && !ectx->is_index)
		r = -APKE_FORMAT_INVALID;
	if (r == 0 && (!sctx.data_verified || !sctx.end_seen)) r = -APKE_V2PKG_INTEGRITY;
	if ((r == 0 || r == -ECANCELED) && sctx.verify_error) r = sctx.verify_error;
	if (r == -APKE_SIGNATURE_UNTRUSTED && sctx.allow_untrusted) r = 0;
	apk_sign_ctx_free(&sctx);
	free(ectx->desc.ptr);
	apk_extract_reset(ectx);

	return r;
}

void apk_extract_v2_control(struct apk_extract_ctx *ectx, apk_blob_t l, apk_blob_t r)
{
	struct apk_sign_ctx *sctx = ectx->pctx;

	if (!sctx || !sctx->control_started || sctx->data_started) return;

	if (apk_blob_compare(APK_BLOB_STR("datahash"), l) == 0) {
		sctx->has_data_checksum = 1;
		sctx->alg = APK_DIGEST_SHA256;
		apk_digest_set(&sctx->data_hash, sctx->alg);
		apk_blob_pull_hexdump(&r, APK_DIGEST_BLOB(sctx->data_hash));
	}
}

int apk_extract_v2_meta(struct apk_extract_ctx *ectx, struct apk_istream *is)
{
	apk_blob_t k, v, token = APK_BLOB_STRLIT("\n");
	while (apk_istream_get_delim(is, token, &k) == 0) {
		if (k.len < 1 || k.ptr[0] == '#') continue;
		if (apk_blob_split(k, APK_BLOB_STRLIT(" = "), &k, &v)) {
			apk_extract_v2_control(ectx, k, v);
		}
	}
	return 0;
}

