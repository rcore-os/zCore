/* app_manifest.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2017 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2017 Timo Teräs <timo.teras@iki.fi>
 * Copyright (C) 2017 William Pitcock <nenolod@dereferenced.org>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <stdio.h>
#include <sys/stat.h>

#include "apk_defines.h"
#include "apk_applet.h"
#include "apk_database.h"
#include "apk_extract.h"
#include "apk_version.h"
#include "apk_print.h"
#include "apk_adb.h"
#include "apk_pathbuilder.h"

/* TODO: support package files as well as generating manifest from the installed DB. */

static void process_package(struct apk_database *db, struct apk_package *pkg)
{
	struct apk_out *out = &db->ctx->out;
	struct apk_installed_package *ipkg = pkg->ipkg;
	const char *prefix1 = "", *prefix2 = "";
	char csum_buf[APK_BLOB_DIGEST_BUF];

	if (ipkg == NULL)
		return;

	if (apk_out_verbosity(out) > 1) {
		prefix1 = pkg->name->name;
		prefix2 = ": ";
	}

	apk_array_foreach_item(diri, ipkg->diris) {
		apk_array_foreach_item(file, diri->files) {
			apk_blob_t csum_blob = APK_BLOB_BUF(csum_buf);
			apk_blob_push_hexdump(&csum_blob, apk_dbf_digest_blob(file));
			csum_blob = apk_blob_pushed(APK_BLOB_BUF(csum_buf), csum_blob);

			apk_out(out, "%s%s%s:" BLOB_FMT "  " DIR_FILE_FMT,
				prefix1, prefix2,
				apk_digest_alg_str(file->digest_alg),
				BLOB_PRINTF(csum_blob),
				DIR_FILE_PRINTF(diri->dir, file));
		}
	}
}

struct manifest_file_ctx {
	struct apk_out *out;
	struct apk_extract_ctx ectx;
	const char *prefix1, *prefix2;
};

static int process_pkg_file(struct apk_extract_ctx *ectx, const struct apk_file_info *fi, struct apk_istream *is)
{
	struct manifest_file_ctx *mctx = container_of(ectx, struct manifest_file_ctx, ectx);
	struct apk_out *out = mctx->out;
	char csum_buf[APK_BLOB_DIGEST_BUF];
	apk_blob_t csum_blob = APK_BLOB_BUF(csum_buf);

	if ((fi->mode & S_IFMT) != S_IFREG) return 0;

	apk_blob_push_hexdump(&csum_blob, APK_DIGEST_BLOB(fi->digest));
	csum_blob = apk_blob_pushed(APK_BLOB_BUF(csum_buf), csum_blob);

	apk_out(out, "%s%s%s:" BLOB_FMT "  %s",
		mctx->prefix1, mctx->prefix2,
		apk_digest_alg_str(fi->digest.alg),
		BLOB_PRINTF(csum_blob),
		fi->name);

	return 0;
}

static int process_v3_meta(struct apk_extract_ctx *ectx, struct adb_obj *pkg)
{
	struct manifest_file_ctx *mctx = container_of(ectx, struct manifest_file_ctx, ectx);
	struct apk_out *out = mctx->out;
	struct adb_obj paths, path, files, file;
	struct apk_digest digest;
	struct apk_pathbuilder pb;
	char buf[APK_DIGEST_LENGTH_MAX*2+1];
	apk_blob_t hex;
	int i, j, n;

	adb_ro_obj(pkg, ADBI_PKG_PATHS, &paths);

	for (i = ADBI_FIRST; i <= adb_ra_num(&paths); i++) {
		adb_ro_obj(&paths, i, &path);
		adb_ro_obj(&path, ADBI_DI_FILES, &files);
		apk_pathbuilder_setb(&pb, adb_ro_blob(&path, ADBI_DI_NAME));

		for (j = ADBI_FIRST; j <= adb_ra_num(&files); j++) {
			adb_ro_obj(&files, j, &file);
			n = apk_pathbuilder_pushb(&pb, adb_ro_blob(&file, ADBI_FI_NAME));
			apk_digest_from_blob(&digest, adb_ro_blob(&file, ADBI_FI_HASHES));

			hex = APK_BLOB_BUF(buf);
			apk_blob_push_hexdump(&hex, APK_DIGEST_BLOB(digest));
			apk_blob_push_blob(&hex, APK_BLOB_STRLIT("\0"));

			apk_out(out, "%s%s%s:%s  %s",
				mctx->prefix1, mctx->prefix2,
				apk_digest_alg_str(digest.alg), buf,
				apk_pathbuilder_cstr(&pb));
			apk_pathbuilder_pop(&pb, n);
		}
	}

	return -ECANCELED;
}

static const struct apk_extract_ops extract_manifest_ops = {
	.v2meta = apk_extract_v2_meta,
	.v3meta = process_v3_meta,
	.file = process_pkg_file,
};

static void process_file(struct apk_database *db, const char *match)
{
	struct apk_out *out = &db->ctx->out;
	struct manifest_file_ctx ctx = {
		.out = out,
		.prefix1 = "",
		.prefix2 = "",
	};
	int r;

	apk_extract_init(&ctx.ectx, db->ctx, &extract_manifest_ops);
	if (apk_out_verbosity(out) > 1) {
		ctx.prefix1 = match;
		ctx.prefix2 = ": ";
	}

	r = apk_extract(&ctx.ectx, apk_istream_from_file(AT_FDCWD, match));
	if (r < 0 && r != -ECANCELED) apk_err(out, "%s: %s", match, apk_error_str(r));
}

static int process_match(struct apk_database *db, const char *match, struct apk_name *name, void *ctx)
{
	if (!name) {
		process_file(db, match);
		return 0;
	}
	apk_name_sorted_providers(name);
	apk_array_foreach(p, name->providers) {
		if (p->pkg->name != name) continue;
		process_package(db, p->pkg);
	}
	return 0;
}

static int manifest_main(void *applet_ctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	if (apk_array_len(args) == 0) return 0;
	apk_db_foreach_sorted_name(ac->db, args, process_match, NULL);
	return 0;
}

static struct apk_applet apk_manifest = {
	.name = "manifest",
	.open_flags = APK_OPENF_READ,
	.main = manifest_main,
};

APK_DEFINE_APPLET(apk_manifest);
