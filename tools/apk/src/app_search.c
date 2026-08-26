/* app_search.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2009 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <fnmatch.h>
#include <stdio.h>
#include "apk_defines.h"
#include "apk_applet.h"
#include "apk_package.h"
#include "apk_database.h"

struct search_ctx {
	void (*print_result)(struct search_ctx *ctx, struct apk_package *pkg);
	void (*print_package)(struct search_ctx *ctx, struct apk_package *pkg);

	int verbosity;
	unsigned int matches;
	struct apk_string_array *filter;
};

static void print_package_name(struct search_ctx *ctx, struct apk_package *pkg)
{
	printf("%s", pkg->name->name);
	if (ctx->verbosity > 0)
		printf("-" BLOB_FMT, BLOB_PRINTF(*pkg->version));
	if (ctx->verbosity > 1)
		printf(" - " BLOB_FMT, BLOB_PRINTF(*pkg->description));
	printf("\n");
}

static void print_origin_name(struct search_ctx *ctx, struct apk_package *pkg)
{
	if (pkg->origin->len)
		printf(BLOB_FMT, BLOB_PRINTF(*pkg->origin));
	else
		printf("%s", pkg->name->name);
	if (ctx->verbosity > 0)
		printf("-" BLOB_FMT, BLOB_PRINTF(*pkg->version));
	printf("\n");
}

static void print_rdep_pkg(struct apk_package *pkg0, struct apk_dependency *dep0, struct apk_package *pkg, void *pctx)
{
	struct search_ctx *ctx = (struct search_ctx *) pctx;
	ctx->print_package(ctx, pkg0);
}

static void print_rdepends(struct search_ctx *ctx, struct apk_package *pkg)
{
	if (ctx->verbosity > 0) {
		ctx->matches = apk_foreach_genid() | APK_DEP_SATISFIES | APK_FOREACH_NO_CONFLICTS;
		printf(PKG_VER_FMT " is required by:\n", PKG_VER_PRINTF(pkg));
	}
	apk_pkg_foreach_reverse_dependency(pkg, ctx->matches, print_rdep_pkg, ctx);
}

#define SEARCH_OPTIONS(OPT) \
	OPT(OPT_SEARCH_all,		APK_OPT_SH("a") "all") \
	OPT(OPT_SEARCH_description,	APK_OPT_SH("d") "description") \
	OPT(OPT_SEARCH_exact,		APK_OPT_SH("e") APK_OPT_SH("x") "exact") \
	OPT(OPT_SEARCH_has_origin,	"has-origin") \
	OPT(OPT_SEARCH_origin,		APK_OPT_SH("o") "origin") \
	OPT(OPT_SEARCH_rdepends,	APK_OPT_SH("r") "rdepends") \

APK_OPTIONS(search_options_desc, SEARCH_OPTIONS);

static int search_parse_option(void *ctx, struct apk_ctx *ac, int opt, const char *optarg)
{
	struct search_ctx *ictx = (struct search_ctx *) ctx;
	struct apk_query_spec *qs = &ac->query;

	switch (opt) {
	case APK_OPTIONS_INIT:
		qs->mode.search = 1;
		qs->mode.empty_matches_all = 1;
		//qs->match = BIT(APK_Q_FIELD_NAME) | BIT(APK_Q_FIELD_PROVIDES);
		break;
	case OPT_SEARCH_all:
		qs->filter.all_matches = 1;
		break;
	case OPT_SEARCH_description:
		qs->match = BIT(APK_Q_FIELD_NAME) | BIT(APK_Q_FIELD_DESCRIPTION);
		qs->mode.search = 1;
		qs->filter.all_matches = 1;
		break;
	case OPT_SEARCH_exact:
		qs->mode.search = 0;
		break;
	case OPT_SEARCH_origin:
		ictx->print_package = print_origin_name;
		break;
	case OPT_SEARCH_rdepends:
		ictx->print_result = print_rdepends;
		break;
	case OPT_SEARCH_has_origin:
		qs->match = BIT(APK_Q_FIELD_ORIGIN);
		qs->filter.all_matches = 1;
		qs->mode.search = 0;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static int search_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_database *db = ac->db;
	struct apk_out *out = &ac->out;
	struct search_ctx *ctx = (struct search_ctx *) pctx;
	struct apk_package_array *pkgs;
	int r;

	ctx->verbosity = apk_out_verbosity(&db->ctx->out);
	ctx->filter = args;
	ctx->matches = apk_foreach_genid() | APK_DEP_SATISFIES | APK_FOREACH_NO_CONFLICTS;
	if (ctx->print_package == NULL)
		ctx->print_package = print_package_name;
	if (ctx->print_result == NULL)
		ctx->print_result = ctx->print_package;

	ac->query.match |= BIT(APK_Q_FIELD_NAME) | BIT(APK_Q_FIELD_PROVIDES);
	apk_package_array_init(&pkgs);
	r = apk_query_packages(ac, &ac->query, args, &pkgs);
	if (r >= 0) {
		apk_array_foreach_item(pkg, pkgs) ctx->print_result(ctx, pkg);
	} else {
		apk_err(out, "query failed: %s", apk_error_str(r));
	}
	apk_package_array_free(&pkgs);

	return r;
}

static struct apk_applet apk_search = {
	.name = "search",
	.options_desc = search_options_desc,
	.optgroup_query = 1,
	.open_flags = APK_OPENF_READ | APK_OPENF_NO_STATE | APK_OPENF_ALLOW_ARCH,
	.context_size = sizeof(struct search_ctx),
	.parse = search_parse_option,
	.main = search_main,
};

APK_DEFINE_APPLET(apk_search);
