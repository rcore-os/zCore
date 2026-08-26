/* app_del.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <stdio.h>
#include "apk_applet.h"
#include "apk_database.h"
#include "apk_print.h"
#include "apk_solver.h"

struct del_ctx {
	struct apk_dependency_array *world;
	unsigned short recursive_delete : 1;
	unsigned int genid;
	int errors;
};

#define DEL_OPTIONS(OPT) \
	OPT(OPT_DEL_redepends,	APK_OPT_SH("r") "rdepends")

APK_OPTIONS(del_options_desc, DEL_OPTIONS);

static int del_parse_option(void *pctx, struct apk_ctx *ac, int opt, const char *optarg)
{
	struct del_ctx *ctx = (struct del_ctx *) pctx;

	switch (opt) {
	case OPT_DEL_redepends:
		ctx->recursive_delete = 1;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

/* struct apk_name.state_int flags */
#define NAME_WAS_WORLD_CONFLICT		BIT(0)
#define NAME_IS_WORLD_DEPENDENCY	BIT(1)

struct not_deleted_ctx {
	struct apk_out *out;
	struct apk_indent indent;
	struct apk_name *name;
	unsigned int matches;
	int header;
	int verbose;
};

static void print_not_deleted_pkg(struct apk_package *pkg0, struct apk_dependency *dep0,
				  struct apk_package *pkg, void *pctx)
{
	struct not_deleted_ctx *ctx = (struct not_deleted_ctx *) pctx;
	struct apk_out *out = ctx->out;

	if (!ctx->header) {
		apk_msg(out, "World updated, but the following packages are not removed due to:");
		ctx->header = 1;
	}
	if (!ctx->indent.indent)
		apk_print_indented_group(&ctx->indent, 0, "  %s:", ctx->name->name);
	if (pkg0->name->state_int & NAME_IS_WORLD_DEPENDENCY)
		apk_print_indented(&ctx->indent, APK_BLOB_STR(pkg0->name->name));
	apk_array_foreach(d, pkg0->provides) {
		if (!(d->name->state_int & NAME_IS_WORLD_DEPENDENCY)) continue;
		apk_print_indented(&ctx->indent, APK_BLOB_STR(d->name->name));
	}

	apk_pkg_foreach_reverse_dependency(pkg0, ctx->matches, print_not_deleted_pkg, pctx);

	// Traverse the providers of the packages automatically installed, limit to the marked world packages (issue #11151)
	apk_array_foreach(d, pkg0->install_if) {
		apk_array_foreach(p, d->name->providers) {
			if (!p->pkg->marked) continue;
			if (apk_pkg_match_genid(p->pkg, ctx->matches)) continue;
			if (!ctx->verbose && !(p->pkg->name->state_int & NAME_IS_WORLD_DEPENDENCY)) continue;
			print_not_deleted_pkg(p->pkg, NULL, NULL, pctx);
		}
	}
}

static int print_not_deleted_name(struct apk_database *db, const char *match,
				  struct apk_name *name, void *pctx)
{
	struct apk_out *out = &db->ctx->out;
	struct not_deleted_ctx *ctx = (struct not_deleted_ctx *) pctx;

	if (!name || (name->state_int & NAME_WAS_WORLD_CONFLICT)) return 0;

	ctx->name = name;
	ctx->matches = apk_foreach_genid() | APK_FOREACH_MARKED | APK_DEP_SATISFIES;
	apk_print_indented_init(&ctx->indent, out, 0);
	apk_array_foreach(p, name->providers)
		if (p->pkg->marked)
			print_not_deleted_pkg(p->pkg, NULL, NULL, ctx);
	apk_print_indented_end(&ctx->indent);
	return 0;
}

static void delete_pkg(struct apk_package *pkg0, struct apk_dependency *dep0,
		       struct apk_package *pkg, void *pctx)
{
	struct del_ctx *ctx = (struct del_ctx *) pctx;

	apk_deps_del(&ctx->world, pkg0->name);
	apk_solver_set_name_flags(pkg0->name, APK_SOLVERF_REMOVE, 0);

	if (ctx->recursive_delete) {
		apk_array_foreach(d, pkg0->provides)
			apk_deps_del(&ctx->world, d->name);

		apk_pkg_foreach_reverse_dependency(
			pkg0, ctx->genid | APK_FOREACH_INSTALLED | APK_DEP_SATISFIES,
			delete_pkg, pctx);
	}
}

static int delete_name(struct apk_database *db, const char *match,
			struct apk_name *name, void *pctx)
{
	struct apk_out *out = &db->ctx->out;
	struct del_ctx *ctx = (struct del_ctx *) pctx;
	struct apk_package *pkg;

	if (!name) {
		apk_err(out, "No such package: %s", match);
		ctx->errors++;
		return 0;
	}

	pkg = apk_pkg_get_installed(name);
	if (pkg != NULL)
		delete_pkg(pkg, NULL, NULL, pctx);
	else
		apk_deps_del(&ctx->world, name);
	return 0;
}

static int del_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_database *db = ac->db;
	struct del_ctx *ctx = (struct del_ctx *) pctx;
	struct not_deleted_ctx ndctx = {
		.out = &db->ctx->out,
		.verbose = apk_out_verbosity(&db->ctx->out) >= 2
	};
	struct apk_changeset changeset = {};
	struct apk_dependency_array *orig_world = apk_array_bclone(db->world, &db->ba_deps);
	int r = 0;

	apk_change_array_init(&changeset.changes);
	ctx->genid = apk_foreach_genid();
	apk_dependency_array_init(&ctx->world);
	apk_dependency_array_copy(&ctx->world, db->world);
	if (apk_array_len(args)) apk_db_foreach_matching_name(db, args, delete_name, ctx);
	if (ctx->errors) return ctx->errors;

	r = apk_solver_solve(db, 0, ctx->world, &changeset);
	if (r == 0) {
		if (apk_out_verbosity(&db->ctx->out) >= 1) {
			apk_array_foreach(d, orig_world)
				if (d->op & APK_VERSION_CONFLICT)
					d->name->state_int |= NAME_WAS_WORLD_CONFLICT;
			apk_array_foreach(d, ctx->world)
				if (!(d->op & APK_VERSION_CONFLICT))
					d->name->state_int |= NAME_IS_WORLD_DEPENDENCY;
			/* check for non-deleted package names */
			apk_array_foreach(change, changeset.changes)
				if (change->new_pkg != NULL)
					change->new_pkg->marked = 1;
			if (apk_array_len(args))
				apk_db_foreach_sorted_name(db, args, print_not_deleted_name, &ndctx);
			if (ndctx.header)
				printf("\n");
		}

		r = apk_solver_commit_changeset(db, &changeset, ctx->world);
	} else {
		apk_solver_print_errors(db, &changeset, ctx->world);
	}
	apk_change_array_free(&changeset.changes);
	apk_dependency_array_free(&ctx->world);

	return r;
}

static struct apk_applet apk_del = {
	.name = "del",
	.options_desc = del_options_desc,
	.optgroup_commit = 1,
	.open_flags = APK_OPENF_WRITE | APK_OPENF_NO_AUTOUPDATE,
	.remove_empty_arguments = 1,
	.context_size = sizeof(struct del_ctx),
	.parse = del_parse_option,
	.main = del_main,
};

APK_DEFINE_APPLET(apk_del);
