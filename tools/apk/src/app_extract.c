/* extract.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2008-2021 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <errno.h>
#include <stdio.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>

#include "apk_applet.h"
#include "apk_print.h"
#include "apk_extract.h"
#include "apk_fs.h"

struct extract_ctx {
	const char *destination;
	unsigned int extract_flags;

	struct apk_extract_ctx ectx;
	struct apk_ctx *ac;
};

#define EXTRACT_OPTIONS(OPT) \
	OPT(OPT_EXTRACT_destination,	APK_OPT_ARG "destination") \
	OPT(OPT_EXTRACT_no_chown,	"no-chown")

APK_OPTIONS(extract_options_desc, EXTRACT_OPTIONS);

static int extract_parse_option(void *pctx, struct apk_ctx *ac, int opt, const char *optarg)
{
	struct extract_ctx *ctx = (struct extract_ctx *) pctx;

	switch (opt) {
	case OPT_EXTRACT_destination:
		ctx->destination = optarg;
		break;
	case OPT_EXTRACT_no_chown:
		ctx->extract_flags |= APK_FSEXTRACTF_NO_CHOWN;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static int extract_v3_meta(struct apk_extract_ctx *ectx, struct adb_obj *pkg)
{
	return 0;
}

static int extract_file(struct apk_extract_ctx *ectx, const struct apk_file_info *fi, struct apk_istream *is)
{
	struct extract_ctx *ctx = container_of(ectx, struct extract_ctx, ectx);
	struct apk_out *out = &ctx->ac->out;
	char buf[APK_EXTRACTW_BUFSZ];
	int r;

	apk_dbg2(out, "%s", fi->name);
	r = apk_fs_extract(ctx->ac, fi, is, ctx->extract_flags, APK_BLOB_NULL);
	if (r > 0) {
		apk_warn(out, "failed to preserve %s: %s",
			fi->name, apk_extract_warning_str(r, buf, sizeof buf));
		r = 0;
	}
	if (r == -EEXIST && S_ISDIR(fi->mode)) r = 0;
	return r;
}

static const struct apk_extract_ops extract_ops = {
	.v2meta = apk_extract_v2_meta,
	.v3meta = extract_v3_meta,
	.file = extract_file,
};

static int extract_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct extract_ctx *ctx = pctx;
	struct apk_out *out = &ac->out;
	int r = 0;

	ctx->ac = ac;
	if (getuid() != 0) ctx->extract_flags |= APK_FSEXTRACTF_NO_CHOWN|APK_FSEXTRACTF_NO_SYS_XATTRS;
	if (!(ac->force & APK_FORCE_OVERWRITE)) ctx->extract_flags |= APK_FSEXTRACTF_NO_OVERWRITE;
	if (!ctx->destination) ctx->destination = ".";

	ac->dest_fd = openat(AT_FDCWD, ctx->destination, O_DIRECTORY | O_RDONLY | O_CLOEXEC);
	if (ac->dest_fd < 0) {
		r = -errno;
		apk_err(out, "Error opening destination '%s': %s",
			ctx->destination, apk_error_str(r));
		return r;
	}

	apk_extract_init(&ctx->ectx, ac, &extract_ops);
	apk_array_foreach_item(arg, args) {
		apk_out(out, "Extracting %s...", arg);
		r = apk_extract(&ctx->ectx, apk_istream_from_fd_url(AT_FDCWD, arg, apk_ctx_since(ac, 0)));
		if (r != 0) {
			apk_err(out, "%s: %s", arg, apk_error_str(r));
			break;
		}
	}
	close(ac->dest_fd);
	return r;
}

static struct apk_applet app_extract = {
	.name = "extract",
	.options_desc = extract_options_desc,
	.context_size = sizeof(struct extract_ctx),
	.parse = extract_parse_option,
	.main = extract_main,
};

APK_DEFINE_APPLET(app_extract);

