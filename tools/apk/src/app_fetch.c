/* app_fetch.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <stdio.h>
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <zlib.h>

#include "apk_applet.h"
#include "apk_database.h"
#include "apk_extract.h"
#include "apk_query.h"

#define FETCH_STDOUT		0x01
#define FETCH_LINK		0x02
#define FETCH_URL		0x04

struct fetch_ctx {
	struct apk_ctx *ac;
	unsigned int flags;
	int outdir_fd, errors;
	time_t built_after;
	apk_blob_t pkgname_spec;
	struct apk_progress prog;
	struct apk_package_array *pkgs;
	unsigned long done_packages;
	uint64_t done_bytes, total_bytes;
};

static int cup(void)
{
	/* compressed/uncompressed size is 259/1213 */
	static unsigned char z[] = {
		0x78,0x9c,0x9d,0x94,0x3d,0x8e,0xc4,0x20,0x0c,0x85,0xfb,0x9c,
		0xc2,0x72,0x43,0x46,0x8a,0x4d,0x3f,0x67,0x89,0x64,0x77,0x2b,
		0x6d,0xbb,0x6d,0x0e,0x3f,0xc6,0x84,0x4d,0x08,0x84,0x55,0xd6,
		0xa2,0xe0,0xef,0x7b,0x36,0xe1,0x11,0x80,0x6e,0xcc,0x53,0x7f,
		0x3e,0xc5,0xeb,0xcf,0x1d,0x20,0x22,0xcc,0x3c,0x53,0x8e,0x17,
		0xd9,0x80,0x6d,0xee,0x0e,0x61,0x42,0x3c,0x8b,0xcf,0xc7,0x12,
		0x22,0x71,0x8b,0x31,0x05,0xd5,0xb0,0x11,0x4b,0xa7,0x32,0x2f,
		0x80,0x69,0x6b,0xb0,0x98,0x40,0xe2,0xcd,0xba,0x6a,0xba,0xe4,
		0x65,0xed,0x61,0x23,0x44,0xb5,0x95,0x06,0x8b,0xde,0x6c,0x61,
		0x70,0xde,0x0e,0xb6,0xed,0xc4,0x43,0x0c,0x56,0x6f,0x8f,0x31,
		0xd0,0x35,0xb5,0xc7,0x58,0x06,0xff,0x81,0x49,0x84,0xb8,0x0e,
		0xb1,0xd8,0xc1,0x66,0x31,0x0e,0x46,0x5c,0x43,0xc9,0xef,0xe5,
		0xdc,0x63,0xb1,0xdc,0x67,0x6d,0x31,0xb3,0xc9,0x69,0x74,0x87,
		0xc7,0xa3,0x1b,0x6a,0xb3,0xbd,0x2f,0x3b,0xd5,0x0c,0x57,0x3b,
		0xce,0x7c,0x5e,0xe5,0x48,0xd0,0x48,0x01,0x92,0x49,0x8b,0xf7,
		0xfc,0x58,0x67,0xb3,0xf7,0x14,0x20,0x5c,0x4c,0x9e,0xcc,0xeb,
		0x78,0x7e,0x64,0xa6,0xa1,0xf5,0xb2,0x70,0x38,0x09,0x7c,0x7f,
		0xfd,0xc0,0x8a,0x4e,0xc8,0x55,0xe8,0x12,0xe2,0x9f,0x1a,0xb1,
		0xb9,0x82,0x52,0x02,0x7a,0xe5,0xf9,0xd9,0x88,0x47,0x79,0x3b,
		0x46,0x61,0x27,0xf9,0x51,0xb1,0x17,0xb0,0x2c,0x0e,0xd5,0x39,
		0x2d,0x96,0x25,0x27,0xd6,0xd1,0x3f,0xa5,0x08,0xe1,0x9e,0x4e,
		0xa7,0xe9,0x03,0xb1,0x0a,0xb6,0x75
	};
	unsigned char buf[1213];
	unsigned long len = sizeof(buf);

	uncompress(buf, &len, z, sizeof(z));
	return write(STDOUT_FILENO, buf, len) != len;
}

#define FETCH_OPTIONS(OPT) \
	OPT(OPT_FETCH_built_after,	APK_OPT_ARG "built-after") \
	OPT(OPT_FETCH_link,		APK_OPT_SH("l") "link") \
	OPT(OPT_FETCH_pkgname_spec,	APK_OPT_ARG "pkgname-spec") \
	OPT(OPT_FETCH_output,		APK_OPT_ARG APK_OPT_SH("o") "output") \
	OPT(OPT_FETCH_simulate,		APK_OPT_BOOL "simulate") \
	OPT(OPT_FETCH_stdout,		APK_OPT_SH("s") "stdout") \
	OPT(OPT_FETCH_url,		"url") \

APK_OPTIONS(fetch_options_desc, FETCH_OPTIONS);

static time_t parse_time(const char *timestr)
{
	struct tm tm;
	char *p;
	time_t t;

	p = strptime(timestr, "%Y-%m-%d %H:%M:%S", &tm);
	if (p && *p == 0) return mktime(&tm);

	t = strtoul(timestr, &p, 10);
	if (p && *p == 0) return t;

	return 0;
}

static int fetch_parse_option(void *ctx, struct apk_ctx *ac, int opt, const char *optarg)
{
	struct fetch_ctx *fctx = (struct fetch_ctx *) ctx;

	switch (opt) {
	case APK_OPTIONS_INIT:
		fctx->outdir_fd = AT_FDCWD;
		break;
	case OPT_FETCH_built_after:
		fctx->built_after = parse_time(optarg);
		if (!fctx->built_after) return -EINVAL;
		break;
	case OPT_FETCH_simulate:
		apk_opt_set_flag(optarg, APK_SIMULATE, &ac->flags);
		break;
	case OPT_FETCH_pkgname_spec:
		fctx->pkgname_spec = APK_BLOB_STR(optarg);
		break;
	case OPT_FETCH_stdout:
		fctx->flags |= FETCH_STDOUT;
		break;
	case OPT_FETCH_link:
		fctx->flags |= FETCH_LINK;
		break;
	case OPT_FETCH_output:
		fctx->outdir_fd = openat(AT_FDCWD, optarg, O_DIRECTORY | O_RDONLY | O_CLOEXEC);
		break;
	case OPT_FETCH_url:
		fctx->flags |= FETCH_URL;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static int fetch_package(struct fetch_ctx *ctx, struct apk_package *pkg)
{
	struct apk_out *out = &ctx->ac->out;
	struct apk_database *db = ctx->ac->db;
	struct apk_istream *is;
	struct apk_ostream *os;
	struct apk_repository *repo;
	struct apk_file_info fi;
	struct apk_extract_ctx ectx;
	struct apk_progress_istream pis;
	char pkg_url[PATH_MAX], filename[PATH_MAX];
	int r, pkg_fd;

	apk_progress_item_start(&ctx->prog, apk_progress_weight(ctx->done_bytes, ctx->done_packages), pkg->size);

	repo = apk_db_select_repo(db, pkg);
	if (repo == NULL) {
		r = -APKE_PACKAGE_NOT_FOUND;
		goto err;
	}

	r = apk_blob_subst(filename, sizeof filename, ctx->pkgname_spec, apk_pkg_subst, pkg);
	if (r < 0) goto err;

	if (!(ctx->flags & FETCH_STDOUT)) {
		if (apk_fileinfo_get(ctx->outdir_fd, filename, 0, &fi, &db->atoms) == 0 &&
		    fi.size == pkg->size)
			goto done;
	}

	r = apk_repo_package_url(db, repo, pkg, &pkg_fd, pkg_url, sizeof pkg_url);
	if (r < 0) goto err;

	if (ctx->flags & FETCH_URL)
		apk_msg(out, "%s", pkg_url);
	else
		apk_msg(out, "Downloading " PKG_VER_FMT, PKG_VER_PRINTF(pkg));

	if (db->ctx->flags & APK_SIMULATE) return 0;

	if (ctx->flags & FETCH_STDOUT) {
		os = apk_ostream_to_fd(STDOUT_FILENO);
	} else {
		if ((ctx->flags & FETCH_LINK) && pkg_fd >= 0) {
			const char *urlfile = apk_url_local_file(pkg_url, PATH_MAX);
			if (urlfile &&
			    linkat(pkg_fd, pkg_url, ctx->outdir_fd, filename, AT_SYMLINK_FOLLOW) == 0)
				goto done;
		}
		os = apk_ostream_to_file_safe(ctx->outdir_fd, filename, 0644);
		if (IS_ERR(os)) {
			r = PTR_ERR(os);
			goto err;
		}
	}

	is = apk_istream_from_fd_url(pkg_fd, pkg_url, apk_db_url_since(db, 0));
	if (IS_ERR(is)) {
		r = PTR_ERR(is);
		goto err;
	}
	is = apk_progress_istream(&pis, is, &ctx->prog);
	is = apk_istream_tee(is, os, APK_ISTREAM_TEE_COPY_META);
	apk_extract_init(&ectx, db->ctx, NULL);
	apk_extract_verify_identity(&ectx, pkg->digest_alg, apk_pkg_digest_blob(pkg));
	r = apk_extract(&ectx, is);
	if (r == 0) goto done;
err:
	apk_err(out, PKG_VER_FMT ": %s", PKG_VER_PRINTF(pkg), apk_error_str(r));
	ctx->errors++;
done:
	ctx->done_bytes += pkg->size;
	ctx->done_packages++;
	apk_progress_item_end(&ctx->prog);
	return 0;
}

static int fetch_match_package(void *pctx, struct apk_query_match *qm)
{
	struct fetch_ctx *ctx = pctx;
	struct apk_out *out = &ctx->ac->out;
	struct apk_package *pkg = qm->pkg;

	if (pkg == NULL) {
		if (apk_blob_contains(qm->query, APK_BLOB_STRLIT("*")) < 0) {
			apk_msg(out, BLOB_FMT ": unable to select package (or its dependencies)",
				BLOB_PRINTF(qm->query));
			ctx->errors++;
		}
		return 0;
	}
	if (ctx->built_after && pkg->build_time && ctx->built_after >= pkg->build_time) return 0;
	ctx->total_bytes += pkg->size;
	apk_package_array_add(&ctx->pkgs, pkg);
	return 0;
}

static int purge_package(void *pctx, int dirfd, const char *path, const char *filename)
{
	struct fetch_ctx *ctx = (struct fetch_ctx *) pctx;
	struct apk_database *db = ctx->ac->db;
	struct apk_out *out = &db->ctx->out;
	struct apk_file_info fi;

	if (apk_fileinfo_get(dirfd, filename, 0, &fi, NULL) == 0) {
		struct apk_package *pkg = apk_db_get_pkg_by_name(db, APK_BLOB_STR(filename), fi.size, ctx->pkgname_spec);
		if (pkg && pkg->marked) return 0;
	}

	apk_msg(out, "Purging %s", filename);
	if (db->ctx->flags & APK_SIMULATE) return 0;
	unlinkat(dirfd, filename, 0);
	return 0;
}

static int fetch_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_out *out = &ac->out;
	struct apk_database *db = ac->db;
	struct fetch_ctx *ctx = (struct fetch_ctx *) pctx;

	ctx->ac = ac;

	if (APK_BLOB_IS_NULL(ctx->pkgname_spec)) ctx->pkgname_spec = ac->default_pkgname_spec;
	if (ctx->flags & FETCH_STDOUT) {
		db->ctx->out.progress = 0;
		db->ctx->out.verbosity = 0;
	}

	if ((apk_array_len(args) == 1) && (strcmp(args->item[0], "coffee") == 0)) {
		if (db->ctx->force) return cup();
		apk_msg(out, "Go and fetch your own coffee.");
		return 0;
	}

	apk_package_array_init(&ctx->pkgs);
	apk_query_matches(ac, &ac->query, args, fetch_match_package, ctx);
	if (ctx->errors == 0) {
		apk_array_qsort(ctx->pkgs, apk_package_array_qsort);
		apk_progress_start(&ctx->prog, &ac->out, "fetch", apk_progress_weight(ctx->total_bytes, apk_array_len(ctx->pkgs)));
		apk_array_foreach_item(pkg, ctx->pkgs)
			fetch_package(ctx, pkg);
		apk_progress_end(&ctx->prog);

		/* Remove packages not matching download spec from the output directory */
		if (!ctx->errors && (db->ctx->flags & APK_PURGE) &&
		    !(ctx->flags & FETCH_STDOUT) && ctx->outdir_fd > 0)
			apk_dir_foreach_file(ctx->outdir_fd, NULL, purge_package, ctx, apk_filename_is_hidden);
	}
	apk_package_array_free(&ctx->pkgs);
	return ctx->errors;
}

static struct apk_applet apk_fetch = {
	.name = "fetch",
	.options_desc = fetch_options_desc,
	.optgroup_query = 1,
	.open_flags = APK_OPENF_READ | APK_OPENF_NO_STATE | APK_OPENF_ALLOW_ARCH,
	.context_size = sizeof(struct fetch_ctx),
	.parse = fetch_parse_option,
	.main = fetch_main,
};

APK_DEFINE_APPLET(apk_fetch);

