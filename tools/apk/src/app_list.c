/* app_list.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2009 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * Copyright (C) 2018 William Pitcock <nenolod@dereferenced.org>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <stdio.h>
#include <unistd.h>
#include "apk_defines.h"
#include "apk_applet.h"
#include "apk_package.h"
#include "apk_database.h"
#include "apk_hash.h"
#include "apk_print.h"

struct match {
	struct apk_name *name;
	struct apk_package *pkg;
};
APK_ARRAY(match_array, struct match *);

struct match_hash_item {
	struct hlist_node hash_node;
	struct match match;
};

static apk_blob_t match_hash_get_key(apk_hash_item item)
{
	struct match_hash_item *m = item;
	return APK_BLOB_STRUCT(m->match);
}

static struct apk_hash_ops match_ops = {
	.node_offset = offsetof(struct match_hash_item, hash_node),
	.get_key = match_hash_get_key,
	.hash_key = apk_blob_hash,
	.compare = apk_blob_compare,
};

struct list_ctx {
	struct apk_balloc *ba;
	struct apk_hash hash;
	struct match_array *matches;
	int verbosity;
	unsigned int match_providers : 1;
	unsigned int match_depends : 1;
	unsigned int manifest : 1;
};

static void print_package(const struct apk_database *db, const struct apk_name *name, const struct apk_package *pkg, const struct list_ctx *ctx)
{
	if (ctx->match_providers) printf("<%s> ", name->name);

	if (ctx->manifest) {
		printf("%s " BLOB_FMT "\n", pkg->name->name, BLOB_PRINTF(*pkg->version));
		return;
	}

	if (ctx->verbosity <= 0) {
		printf("%s\n", pkg->name->name);
		return;
	}

	printf(PKG_VER_FMT " " BLOB_FMT " ",
		PKG_VER_PRINTF(pkg), BLOB_PRINTF(*pkg->arch));

	if (pkg->origin->len)
		printf("{" BLOB_FMT "}", BLOB_PRINTF(*pkg->origin));
	else
		printf("{%s}", pkg->name->name);

	printf(" (" BLOB_FMT ")", BLOB_PRINTF(*pkg->license));

	if (pkg->ipkg)
		printf(" [installed]");
	else {
		const struct apk_package *u = apk_db_pkg_upgradable(db, pkg);
		if (u != NULL) printf(" [upgradable from: " PKG_VER_FMT "]", PKG_VER_PRINTF(u));
	}

	if (ctx->verbosity > 1) {
		printf("\n  " BLOB_FMT "\n", BLOB_PRINTF(*pkg->description));
		if (ctx->verbosity > 2)
			printf("  <"BLOB_FMT">\n", BLOB_PRINTF(*pkg->url));
	}

	printf("\n");
}

#define LIST_OPTIONS(OPT) \
	OPT(OPT_LIST_available,		APK_OPT_SH("a")) \
	OPT(OPT_LIST_depends,		APK_OPT_SH("d") "depends") \
	OPT(OPT_LIST_installed,		APK_OPT_SH("I")) \
	OPT(OPT_LIST_manifest,		"manifest") \
	OPT(OPT_LIST_origin,		APK_OPT_SH("o") "origin") \
	OPT(OPT_LIST_orphaned,		APK_OPT_SH("O")) \
	OPT(OPT_LIST_providers,		APK_OPT_SH("P") "providers") \
	OPT(OPT_LIST_upgradeable,	APK_OPT_SH("u") "upgradeable")

APK_OPTIONS(list_options_desc, LIST_OPTIONS);

static int list_parse_option(void *pctx, struct apk_ctx *ac, int opt, const char *optarg)
{
	struct list_ctx *ctx = pctx;
	struct apk_query_spec *qs = &ac->query;

	switch (opt) {
	case OPT_LIST_available:
		qs->filter.available = 1;
		break;
	case OPT_LIST_depends:
		ctx->match_depends = 1;
		break;
	case OPT_LIST_installed:
	installed:
		qs->filter.installed = 1;
		ac->open_flags |= APK_OPENF_NO_SYS_REPOS;
		break;
	case OPT_LIST_manifest:
		ctx->manifest = 1;
		goto installed;
	case OPT_LIST_origin:
		qs->match = BIT(APK_Q_FIELD_ORIGIN);
		break;
	case OPT_LIST_orphaned:
		qs->filter.orphaned = 1;
		break;
	case OPT_LIST_providers:
		ctx->match_providers = 1;
		break;
	case OPT_LIST_upgradeable:
		qs->filter.upgradable = 1;
		break;
	default:
		return -ENOTSUP;
	}

	return 0;
}

static int match_array_sort(const void *a, const void *b)
{
	const struct match *ma = *(const struct match **)a, *mb = *(const struct match **)b;
	int r = apk_name_cmp_display(ma->name, mb->name);
	if (r) return r;
	return apk_pkg_cmp_display(ma->pkg, mb->pkg);
}

static int list_match_cb(void *pctx, struct apk_query_match *qm)
{
	struct list_ctx *ctx = pctx;
	struct match m = { .name = qm->name, .pkg = qm->pkg };

	if (!m.pkg) return 0;
	if (!m.name) m.name = m.pkg->name;

	unsigned long hash = apk_hash_from_key(&ctx->hash, APK_BLOB_STRUCT(m));
	if (apk_hash_get_hashed(&ctx->hash, APK_BLOB_STRUCT(m), hash) != NULL) return 0;

	struct match_hash_item *hi = apk_balloc_new(ctx->ba, struct match_hash_item);
	hi->match = m;
	apk_hash_insert_hashed(&ctx->hash, hi, hash);
	match_array_add(&ctx->matches, &hi->match);
	return 0;
}

static int list_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_out *out = &ac->out;
	struct apk_database *db = ac->db;
	struct apk_query_spec *qs = &ac->query;
	struct list_ctx *ctx = pctx;

	ctx->ba = &ac->ba;
	ctx->verbosity = apk_out_verbosity(out);

	qs->mode.empty_matches_all = 1;
	qs->filter.all_matches = 1;
	if (!qs->match) {
		if (ctx->match_depends) qs->match = BIT(APK_Q_FIELD_DEPENDS);
		else if (ctx->match_providers) qs->match = BIT(APK_Q_FIELD_NAME) | BIT(APK_Q_FIELD_PROVIDES);
		else qs->match = BIT(APK_Q_FIELD_NAME);
	}

	apk_hash_init(&ctx->hash, &match_ops, 100);
	match_array_init(&ctx->matches);
	apk_query_matches(ac, qs, args, list_match_cb, ctx);
	apk_array_qsort(ctx->matches, match_array_sort);
	apk_array_foreach_item(m, ctx->matches) print_package(db, m->name, m->pkg, ctx);
	match_array_free(&ctx->matches);
	apk_hash_free(&ctx->hash);
	return 0;
}

static struct apk_applet apk_list = {
	.name = "list",
	.open_flags = APK_OPENF_READ | APK_OPENF_ALLOW_ARCH,
	.options_desc = list_options_desc,
	.optgroup_query = 1,
	.context_size = sizeof(struct list_ctx),
	.parse = list_parse_option,
	.main = list_main,
};

APK_DEFINE_APPLET(apk_list);
