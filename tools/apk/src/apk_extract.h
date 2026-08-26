/* apk_extract.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2021 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once

#include "apk_crypto.h"
#include "apk_print.h"
#include "apk_io.h"

struct adb_obj;
struct apk_ctx;
struct apk_extract_ctx;

struct apk_extract_ops {
	int (*v2index)(struct apk_extract_ctx *, apk_blob_t *desc, struct apk_istream *is);
	int (*v2meta)(struct apk_extract_ctx *, struct apk_istream *is);
	int (*v3index)(struct apk_extract_ctx *, struct adb_obj *);
	int (*v3meta)(struct apk_extract_ctx *, struct adb_obj *);
	int (*script)(struct apk_extract_ctx *, unsigned int script, uint64_t size, struct apk_istream *is);
	int (*file)(struct apk_extract_ctx *, const struct apk_file_info *fi, struct apk_istream *is);
};

struct apk_extract_ctx {
	struct apk_ctx *ac;
	const struct apk_extract_ops *ops;
	struct apk_digest *generate_identity;
	uint8_t generate_alg, verify_alg;
	apk_blob_t verify_digest;
	apk_blob_t desc;
	void *pctx;
	unsigned is_package : 1;
	unsigned is_index : 1;
};

#define APK_EXTRACTW_OWNER		0x0001
#define APK_EXTRACTW_PERMISSION		0x0002
#define APK_EXTRACTW_MTIME		0x0004
#define APK_EXTRACTW_XATTR		0x0008

static inline void apk_extract_init(struct apk_extract_ctx *ectx, struct apk_ctx *ac, const struct apk_extract_ops *ops) {
	*ectx = (struct apk_extract_ctx){.ac = ac, .ops = ops};
}
static inline void apk_extract_reset(struct apk_extract_ctx *ectx) {
	apk_extract_init(ectx, ectx->ac, ectx->ops);
}
static inline void apk_extract_generate_identity(struct apk_extract_ctx *ctx, uint8_t alg, struct apk_digest *id) {
	ctx->generate_alg = alg;
	ctx->generate_identity = id;
}
static inline void apk_extract_verify_identity(struct apk_extract_ctx *ctx, uint8_t alg, apk_blob_t digest) {
	ctx->verify_alg = alg;
	ctx->verify_digest = digest;
}
int apk_extract(struct apk_extract_ctx *, struct apk_istream *is);

#define APK_EXTRACTW_BUFSZ 128
const char *apk_extract_warning_str(int warnings, char *buf, size_t sz);

int apk_extract_v2(struct apk_extract_ctx *, struct apk_istream *is);
void apk_extract_v2_control(struct apk_extract_ctx *, apk_blob_t, apk_blob_t);
int apk_extract_v2_meta(struct apk_extract_ctx *ectx, struct apk_istream *is);

int apk_extract_v3(struct apk_extract_ctx *, struct apk_istream *is);
