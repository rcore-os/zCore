/* app_upgrade.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include "apk_applet.h"
#include "apk_database.h"
#include "apk_print.h"
#include "apk_solver.h"

// APK_PREUPGRADE_TOKEN is used to determine if APK version changed
// so much after self-upgrade that a repository autoupdate should be
// enabled. Mainly needed if the index cache name changes.
#define APK_PREUPGRADE_TOKEN	"laiNgeiThu6ip1Te"

struct upgrade_ctx {
	unsigned short solver_flags;
	unsigned short preupgrade : 1;
	unsigned short preupgrade_only : 1;
	unsigned short ignore : 1;
	unsigned short prune : 1;
	int errors;
};

#define UPGRADE_OPTIONS(OPT) \
	OPT(OPT_UPGRADE_available,		APK_OPT_SH("a") "available") \
	OPT(OPT_UPGRADE_ignore,			"ignore") \
	OPT(OPT_UPGRADE_latest,			APK_OPT_SH("l") "latest") \
	OPT(OPT_UPGRADE_preupgrade,		APK_OPT_BOOL "preupgrade") \
	OPT(OPT_UPGRADE_preupgrade_only,	"preupgrade-only") \
	OPT(OPT_UPGRADE_prune,			"prune") \
	OPT(OPT_UPGRADE_self_upgrade,		APK_OPT_BOOL "self-upgrade") \
	OPT(OPT_UPGRADE_self_upgrade_only,	"self-upgrade-only")

APK_OPTIONS(upgrade_options_desc, UPGRADE_OPTIONS);

static int upgrade_parse_option(void *ctx, struct apk_ctx *ac, int opt, const char *optarg)
{
	struct upgrade_ctx *uctx = (struct upgrade_ctx *) ctx;
	const char *token;

	switch (opt) {
	case APK_OPTIONS_INIT:
		uctx->preupgrade = 1;
		token = getenv("APK_PREUPGRADE_TOKEN");
		if (!token) token = getenv("APK_SELFUPGRADE_TOKEN");
		if (token != NULL && strcmp(token, APK_PREUPGRADE_TOKEN) == 0) {
			uctx->preupgrade = 0;
			ac->open_flags |= APK_OPENF_NO_AUTOUPDATE;
		}
		break;
	case OPT_UPGRADE_preupgrade:
	case OPT_UPGRADE_self_upgrade:
		uctx->preupgrade = APK_OPTARG_VAL(optarg);
		break;
	case OPT_UPGRADE_preupgrade_only:
	case OPT_UPGRADE_self_upgrade_only:
		uctx->preupgrade_only = 1;
		break;
	case OPT_UPGRADE_ignore:
		uctx->ignore = 1;
		break;
	case OPT_UPGRADE_prune:
		uctx->prune = 1;
		break;
	case OPT_UPGRADE_available:
		uctx->solver_flags |= APK_SOLVERF_AVAILABLE;
		break;
	case OPT_UPGRADE_latest:
		uctx->solver_flags |= APK_SOLVERF_LATEST;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

int apk_do_preupgrade(struct apk_database *db, unsigned short solver_flags, unsigned int preupgrade_only)
{
	struct apk_ctx *ac = db->ctx;
	struct apk_out *out = &db->ctx->out;
	struct apk_changeset changeset = {};
	struct apk_dependency_array *deps;
	char buf[PATH_MAX];
	int r = 0;

	apk_dependency_array_init(&deps);
	apk_change_array_init(&changeset.changes);

	struct apk_query_match qm;
	apk_query_who_owns(db, "/proc/self/exe", &qm, buf, sizeof buf);
	if (qm.pkg) {
		apk_deps_add(&deps, &(struct apk_dependency){
			.name = qm.pkg->name,
			.op = APK_DEPMASK_ANY,
			.version = &apk_atom_null,
		});
	}
	apk_array_foreach_item(str, ac->preupgrade_deps) {
		int warn = 0;
		apk_blob_t b = APK_BLOB_STR(str);
		while (b.len > 0) {
			struct apk_dependency dep;
			apk_blob_pull_dep(&b, db, &dep, false);
			if (dep.name) apk_deps_add(&deps, &dep);
			else warn = 1;
		}
		if (warn) apk_warn(out, "Ignored invalid preupgrade dependencies from: %s", str);
	}

	/* Determine if preupgrade can be made */
	apk_array_foreach(dep, deps) {
		struct apk_name *name = dep->name;
		struct apk_package *pkg = apk_pkg_get_installed(name);
		if (!apk_dep_is_materialized(dep, pkg)) continue;
		apk_array_foreach(p0, name->providers) {
			struct apk_package *pkg0 = p0->pkg;
			if (pkg0->repos == 0) continue;
			if (!apk_version_match(*pkg0->version, APK_VERSION_GREATER, *pkg->version))
				continue;
			apk_solver_set_name_flags(name, solver_flags, 0);
			r = 1;
			break;
		}
	}
	if (r == 0) goto ret;

	/* Create new commit for preupgrades with minimal other changes */
	db->performing_preupgrade = 1;

	r = apk_solver_solve(db, 0, db->world, &changeset);
	if (r != 0) {
		apk_warn(out, "Failed to perform initial preupgrade, continuing with a full upgrade.");
		r = 0;
		goto ret;
	}

	if (changeset.num_total_changes == 0)
		goto ret;

	if (!preupgrade_only && db->ctx->flags & APK_SIMULATE) {
		apk_warn(out, "This simulation might not reliable as a preupgrade is available.");
		goto ret;
	}

	if (preupgrade_only) db->performing_preupgrade = 0;

	apk_msg(out, "Preupgrading:");
	r = apk_solver_commit_changeset(db, &changeset, db->world);
	if (r < 0 || preupgrade_only) goto ret;

	apk_db_close(db);

	apk_msg(out, "Continuing with the main upgrade transaction:");
	putenv("APK_PREUPGRADE_TOKEN=" APK_PREUPGRADE_TOKEN);
	putenv("APK_SELFUPGRADE_TOKEN=" APK_PREUPGRADE_TOKEN);

	extern int apk_argc;
	extern char **apk_argv;
	char **argv = malloc(sizeof(char*[apk_argc+2]));
	memcpy(argv, apk_argv, sizeof(char*[apk_argc]));
	argv[apk_argc] = "--no-self-upgrade";
	argv[apk_argc+1] = NULL;
	execvp(argv[0], argv);
	apk_err(out, "PANIC! Failed to re-execute new apk-tools!");
	exit(1);

ret:
	apk_change_array_free(&changeset.changes);
	apk_dependency_array_free(&deps);
	db->performing_preupgrade = 0;
	return r;
}

static int set_upgrade_for_name(struct apk_database *db, const char *match, struct apk_name *name, void *pctx)
{
	struct apk_out *out = &db->ctx->out;
	struct upgrade_ctx *uctx = (struct upgrade_ctx *) pctx;

	if (!name) {
		apk_err(out, "Package '%s' not found", match);
		uctx->errors++;
		return 0;
	}

	apk_solver_set_name_flags(name, uctx->ignore ? APK_SOLVERF_INSTALLED : APK_SOLVERF_UPGRADE, 0);
	return 0;
}

static int upgrade_main(void *ctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_out *out = &ac->out;
	struct apk_database *db = ac->db;
	struct upgrade_ctx *uctx = (struct upgrade_ctx *) ctx;
	unsigned short solver_flags;
	struct apk_dependency_array *world;
	int r = 0;

	apk_dependency_array_init(&world);
	if (apk_db_check_world(db, db->world) != 0) {
		apk_err(out, "Not continuing with upgrade due to missing repository tags.");
		return -1;
	}
	if (apk_db_repository_check(db) != 0) return -1;

	solver_flags = APK_SOLVERF_UPGRADE | uctx->solver_flags;
	if ((uctx->preupgrade_only || !ac->root_set) && uctx->preupgrade && apk_array_len(args) == 0) {
		r = apk_do_preupgrade(db, solver_flags, uctx->preupgrade_only);
		if (r != 0)
			return r;
	}
	if (uctx->preupgrade_only)
		return 0;

	if (uctx->prune || (solver_flags & APK_SOLVERF_AVAILABLE)) {
		apk_dependency_array_copy(&world, db->world);
		if (solver_flags & APK_SOLVERF_AVAILABLE) {
			apk_array_foreach(dep, world) {
				if (dep->op == APK_DEPMASK_CHECKSUM) {
					dep->op = APK_DEPMASK_ANY;
					dep->version = &apk_atom_null;
				}
			}
		}
		if (uctx->prune) {
			int i, j;
			for (i = j = 0; i < apk_array_len(world); i++) {
				apk_array_foreach(p, world->item[i].name->providers) {
					if (apk_db_pkg_available(db, p->pkg)) {
						world->item[j++] = world->item[i];
						break;
					}
				}
			}
			apk_array_truncate(world, j);
		}
	} else {
		world = db->world;
	}

	if (apk_array_len(args) > 0) {
		/* if specific packages are listed, we don't want to upgrade world. */
		if (!uctx->ignore) solver_flags &= ~APK_SOLVERF_UPGRADE;
		apk_db_foreach_matching_name(db, args, set_upgrade_for_name, uctx);
		if (uctx->errors) return uctx->errors;
	}

	r = apk_solver_commit(db, solver_flags, world);

	if (world != db->world) apk_dependency_array_free(&world);
	return r;
}

static struct apk_applet apk_upgrade = {
	.name = "upgrade",
	.options_desc = upgrade_options_desc,
	.optgroup_commit = 1,
	.open_flags = APK_OPENF_WRITE,
	.context_size = sizeof(struct upgrade_ctx),
	.parse = upgrade_parse_option,
	.main = upgrade_main,
};

APK_DEFINE_APPLET(apk_upgrade);
