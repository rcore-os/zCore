/* app_dot.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <stdio.h>
#include <fnmatch.h>

#include "apk_applet.h"
#include "apk_database.h"
#include "apk_print.h"

#define S_EVALUATED	-1
#define S_EVALUATING	-2

struct dot_ctx {
	struct apk_query_spec *qs;
	unsigned short not_empty : 1;
	unsigned short errors_only : 1;
};

#define DOT_OPTIONS(OPT) \
	OPT(OPT_DOT_errors,	"errors")

APK_OPTIONS(dot_options_desc, DOT_OPTIONS);

static int dot_parse_option(void *pctx, struct apk_ctx *ac, int opt, const char *optarg)
{
	struct dot_ctx *ctx = (struct dot_ctx *) pctx;

	switch (opt) {
	case OPT_DOT_errors:
		ctx->errors_only = 1;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static void start_graph(struct dot_ctx *ctx)
{
	if (ctx->not_empty)
		return;
	ctx->not_empty = 1;

	printf( "digraph \"apkindex\" {\n"
		"  rankdir=LR;\n"
		"  node [shape=box];\n");
}

static void dump_error_name(struct dot_ctx *ctx, struct apk_name *name)
{
	if (name->state_int)
		return;
	name->state_int = 1;
	start_graph(ctx);
	printf("  \"%s\" [style=dashed, color=red, fontcolor=red, shape=octagon];\n",
		name->name);
}

static void dump_broken_deps(struct dot_ctx *ctx, struct apk_package *pkg, const char *kind, struct apk_dependency *dep)
{
	if (!dep->broken) return;

	dump_error_name(ctx, dep->name);
	printf("  \"" PKG_VER_FMT "\" -> \"%s\" [arrowhead=%s,style=dashed,color=red,fontcolor=red,label=\"" DEP_FMT "\"];\n",
		PKG_VER_PRINTF(pkg), dep->name->name,
		kind,
		DEP_PRINTF(dep));
}

static int dump_pkg(struct dot_ctx *ctx, struct apk_package *pkg)
{
	struct apk_query_spec *qs = ctx->qs;
	int r, ret = 0;

	if (pkg->state_int == S_EVALUATED)
		return 0;

	if (pkg->state_int <= S_EVALUATING) {
		pkg->state_int--;
		return 1;
	}

	pkg->state_int = S_EVALUATING;
	apk_array_foreach(dep, pkg->depends) {
		struct apk_name *name = dep->name;

		dump_broken_deps(ctx, pkg, "normal", dep);

		if (dep->op & APK_VERSION_CONFLICT)
			continue;

		if (apk_array_len(name->providers) == 0) {
			dump_error_name(ctx, name);
			printf("  \"" PKG_VER_FMT "\" -> \"%s\" [color=red];\n",
				PKG_VER_PRINTF(pkg), name->name);
			continue;
		}

		apk_array_foreach(p0, name->providers) {
			if (qs->filter.installed && !p0->pkg->ipkg) continue;
			if (!apk_dep_is_provided(pkg, dep, p0)) continue;

			r = dump_pkg(ctx, p0->pkg);
			ret += r;
			if (r || (!ctx->errors_only)) {
				start_graph(ctx);

				printf("  \"" PKG_VER_FMT "\" -> \"" PKG_VER_FMT "\"[",
					PKG_VER_PRINTF(pkg),
					PKG_VER_PRINTF(p0->pkg));
				if (r)
					printf("color=red,");
				if (p0->pkg->name != dep->name)
					printf("arrowhead=inv,label=\"%s\",", dep->name->name);
				printf("];\n");
			}
		}
	}
	apk_array_foreach(dep, pkg->provides) dump_broken_deps(ctx, pkg, "inv", dep);
	apk_array_foreach(dep, pkg->install_if) dump_broken_deps(ctx, pkg, "diamond", dep);
	ret -= S_EVALUATING - pkg->state_int;
	pkg->state_int = S_EVALUATED;

	return ret;
}

static int dot_match(void *pctx, struct apk_query_match *qm)
{
	struct dot_ctx *ctx = pctx;

	if (qm->pkg) dump_pkg(ctx, qm->pkg);
	return 0;
}

static int dot_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct dot_ctx *ctx = (struct dot_ctx *) pctx;
	struct apk_query_spec *qs = &ac->query;

	ctx->qs = qs;
	qs->match |= BIT(APK_Q_FIELD_NAME) | BIT(APK_Q_FIELD_PROVIDES);
	qs->mode.empty_matches_all = 1;
	apk_query_matches(ac, qs, args, dot_match, ctx);
	if (!ctx->not_empty) return 1;

	printf("}\n");
	return 0;
}

static struct apk_applet apk_dot = {
	.name = "dot",
	.open_flags = APK_OPENF_READ | APK_OPENF_NO_STATE | APK_OPENF_ALLOW_ARCH,
	.options_desc = dot_options_desc,
	.optgroup_query = 1,
	.remove_empty_arguments = 1,
	.context_size = sizeof(struct dot_ctx),
	.parse = dot_parse_option,
	.main = dot_main,
};

APK_DEFINE_APPLET(apk_dot);
