/* app_fix.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <errno.h>
#include <stdio.h>
#include "apk_applet.h"
#include "apk_database.h"
#include "apk_print.h"
#include "apk_solver.h"
#include "apk_fs.h"

struct fix_ctx {
	struct apk_database *db;
	unsigned short solver_flags;
	unsigned short fix_depends : 1;
	unsigned short fix_xattrs : 1;
	unsigned short fix_directory_permissions : 1;
	int errors;
};

#define FIX_OPTIONS(OPT) \
	OPT(OPT_FIX_depends,			APK_OPT_SH("d") "depends") \
	OPT(OPT_FIX_directory_permissions,	"directory-permissions") \
	OPT(OPT_FIX_reinstall,			APK_OPT_SH("r") "reinstall") \
	OPT(OPT_FIX_upgrade,			APK_OPT_SH("u") "upgrade") \
	OPT(OPT_FIX_xattr,			APK_OPT_SH("x") "xattr")

APK_OPTIONS(fix_options_desc, FIX_OPTIONS);

static int fix_parse_option(void *pctx, struct apk_ctx *ac, int opt, const char *optarg)
{
	struct fix_ctx *ctx = (struct fix_ctx *) pctx;
	switch (opt) {
	case OPT_FIX_depends:
		ctx->fix_depends = 1;
		break;
	case OPT_FIX_directory_permissions:
		ctx->fix_directory_permissions = 1;
		break;
	case OPT_FIX_reinstall:
		ctx->solver_flags |= APK_SOLVERF_REINSTALL;
		break;
	case OPT_FIX_upgrade:
		ctx->solver_flags |= APK_SOLVERF_UPGRADE;
		break;
	case OPT_FIX_xattr:
		ctx->fix_xattrs = 1;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static int fix_directory_permissions(apk_hash_item item, void *pctx)
{
	struct fix_ctx *ctx = (struct fix_ctx *) pctx;
	struct apk_database *db = ctx->db;
	struct apk_out *out = &db->ctx->out;
	struct apk_db_dir *dir = (struct apk_db_dir *) item;

	if (dir->namelen == 0 || !dir->refs) return 0;

	apk_db_dir_prepare(db, dir, dir->owner->acl, dir->owner->acl);
	if (dir->permissions_ok) return 0;

	apk_dbg(out, "fixing directory %s", dir->name);
	dir->permissions_ok = 1;
	apk_db_dir_update_permissions(db, dir->owner);
	return 0;
}

static void mark_fix(struct fix_ctx *ctx, struct apk_name *name)
{
	apk_solver_set_name_flags(name, ctx->solver_flags, ctx->fix_depends ? ctx->solver_flags : 0);
}

static int set_solver_flags(struct apk_database *db, const char *match, struct apk_name *name, void *pctx)
{
	struct apk_out *out = &db->ctx->out;
	struct fix_ctx *ctx = pctx;

	if (!name) {
		apk_err(out, "Package '%s' not found", match);
		ctx->errors++;
		return 0;
	}

	mark_fix(ctx, name);
	return 0;
}

static int fix_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_database *db = ac->db;
	struct fix_ctx *ctx = (struct fix_ctx *) pctx;
	struct apk_installed_package *ipkg;

	ctx->db = db;
	if (!ctx->solver_flags)
		ctx->solver_flags = APK_SOLVERF_REINSTALL;

	if (apk_array_len(args) == 0) {
		list_for_each_entry(ipkg, &db->installed.packages, installed_pkgs_list) {
			if (ipkg->broken_files || ipkg->broken_script ||
			    (ipkg->broken_xattr && ctx->fix_xattrs))
				mark_fix(ctx, ipkg->pkg->name);
		}
	} else
		apk_db_foreach_matching_name(db, args, set_solver_flags, ctx);

	if (ctx->errors) return ctx->errors;

	if (ctx->fix_directory_permissions) {
		apk_hash_foreach(&db->installed.dirs, fix_directory_permissions, ctx);
		if (db->num_dir_update_errors) {
			apk_err(&ac->out, "Failed to fix directory permissions");
			return -1;
		}
	}

	return apk_solver_commit(db, 0, db->world);
}

static struct apk_applet apk_fix = {
	.name = "fix",
	.options_desc = fix_options_desc,
	.optgroup_commit = 1,
	.open_flags = APK_OPENF_WRITE,
	.remove_empty_arguments = 1,
	.context_size = sizeof(struct fix_ctx),
	.parse = fix_parse_option,
	.main = fix_main,
};

APK_DEFINE_APPLET(apk_fix);
