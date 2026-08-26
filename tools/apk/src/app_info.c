/* app_info.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2009 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
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
#include "apk_print.h"

struct info_ctx {
	struct apk_database *db;
	unsigned int who_owns : 1;
	unsigned int exists_test : 1;
	unsigned int partial_result : 1;
};

static int verbosity;

static void info_print_pkg_oneline(struct apk_package *pkg, int minimal_verbosity)
{
	int v = max(verbosity, minimal_verbosity);
	if (pkg == NULL || v < 1) return;
	printf("%s", pkg->name->name);
	if (v > 1) printf("-" BLOB_FMT, BLOB_PRINTF(*pkg->version));
	if (v > 2) printf(" - " BLOB_FMT, BLOB_PRINTF(*pkg->description));
	printf("\n");
}

static int info_exists(struct info_ctx *ctx, struct apk_database *db, struct apk_string_array *args)
{
	struct apk_name *name;
	struct apk_dependency dep;
	int ok, errors = 0;

	apk_array_foreach_item(arg, args) {
		apk_blob_t b = APK_BLOB_STR(arg);

		apk_blob_pull_dep(&b, db, &dep, true);
		if (APK_BLOB_IS_NULL(b) || b.len > 0) continue;

		name = dep.name;
		if (name == NULL) continue;

		ok = apk_dep_is_provided(NULL, &dep, NULL);
		apk_array_foreach(p, name->providers) {
			if (!p->pkg->ipkg) continue;
			ok = apk_dep_is_provided(NULL, &dep, p);
			if (ok) info_print_pkg_oneline(p->pkg, 0);
			break;
		}
		if (!ok) errors++;
	}
	return errors;
}

static int info_who_owns(struct info_ctx *ctx, struct apk_database *db, struct apk_string_array *args)
{
	struct apk_out *out = &db->ctx->out;
	struct apk_query_spec *qs = &db->ctx->query;
	struct apk_package_array *pkgs;
	struct apk_serializer *ser = NULL;
	struct apk_query_match qm;
	char fnbuf[PATH_MAX], buf[PATH_MAX];
	int errors = 0;

	if (qs->ser != &apk_serializer_query) {
		if (!qs->fields) qs->fields = BIT(APK_Q_FIELD_QUERY) | BIT(APK_Q_FIELD_PATH_TARGET) | BIT(APK_Q_FIELD_ERROR) | BIT(APK_Q_FIELD_NAME);
		ser = apk_serializer_init_alloca(db->ctx, qs->ser, apk_ostream_to_fd(STDOUT_FILENO));
		if (IS_ERR(ser)) return PTR_ERR(ser);
		apk_ser_start_array(ser, apk_array_len(args));
	}
	apk_package_array_init(&pkgs);
	apk_array_foreach_item(arg, args) {
		char *fn = arg;
		if (arg[0] != '/' && realpath(arg, fnbuf)) fn = fnbuf;
		apk_query_who_owns(db, fn, &qm, buf, sizeof buf);
		if (ser) {
			apk_ser_start_object(ser);
			apk_query_match_serialize(&qm, db, qs, ser);
			apk_ser_end(ser);
			continue;
		}
		if (!qm.pkg) {
			apk_err(out, "%s: Could not find owner package", fn);
			errors++;
			continue;
		}
		if (verbosity >= 1) {
			printf("%s %sis owned by " PKG_VER_FMT "\n",
			       fn, qm.path_target.ptr ? "symlink target " : "",
			       PKG_VER_PRINTF(qm.pkg));
		} else if (!qm.pkg->marked) {
			qm.pkg->marked = 1;
			apk_package_array_add(&pkgs, qm.pkg);
		}
	}
	if (apk_array_len(pkgs) != 0) {
		apk_array_qsort(pkgs, apk_package_array_qsort);
		apk_array_foreach_item(pkg, pkgs) printf("%s\n", pkg->name->name);
	}
	apk_package_array_free(&pkgs);
	if (ser) {
		apk_ser_end(ser);
		apk_serializer_cleanup(ser);
	}
	return errors;
}

static void info_print_blob(struct apk_database *db, struct apk_package *pkg, const char *field, apk_blob_t value)
{
	if (verbosity > 1)
		printf("%s: " BLOB_FMT "\n", pkg->name->name, BLOB_PRINTF(value));
	else
		printf(PKG_VER_FMT " %s:\n" BLOB_FMT "\n\n", PKG_VER_PRINTF(pkg), field, BLOB_PRINTF(value));
}

static void info_print_size(struct apk_database *db, struct apk_package *pkg)
{
	char buf[64];
	apk_blob_t fmt = apk_fmt_human_size(buf, sizeof buf, pkg->installed_size, -1);
	if (verbosity > 1)
		printf("%s: " BLOB_FMT "\n", pkg->name->name, BLOB_PRINTF(fmt));
	else
		printf(PKG_VER_FMT " installed size:\n" BLOB_FMT "\n\n",
		       PKG_VER_PRINTF(pkg), BLOB_PRINTF(fmt));
}

static void info_print_dep_array(struct apk_database *db, struct apk_package *pkg,
				 struct apk_dependency_array *deps, const char *dep_text)
{
	apk_blob_t separator = APK_BLOB_STR(verbosity > 1 ? " " : "\n");
	char buf[256];

	if (verbosity == 1) printf(PKG_VER_FMT " %s:\n", PKG_VER_PRINTF(pkg), dep_text);
	if (verbosity > 1) printf("%s: ", pkg->name->name);
	apk_array_foreach(d, deps) {
		apk_blob_t b = APK_BLOB_BUF(buf);
		apk_blob_push_dep(&b, db, d);
		apk_blob_push_blob(&b, separator);
		b = apk_blob_pushed(APK_BLOB_BUF(buf), b);
		fwrite(b.ptr, b.len, 1, stdout);
	}
	puts("");
}

static void print_rdep_pkg(struct apk_package *pkg0, struct apk_dependency *dep0, struct apk_package *pkg, void *pctx)
{
	printf(PKG_VER_FMT "%s", PKG_VER_PRINTF(pkg0), verbosity > 1 ? " " : "\n");
}

static void info_print_required_by(struct apk_database *db, struct apk_package *pkg)
{
	if (verbosity == 1) printf(PKG_VER_FMT " is required by:\n", PKG_VER_PRINTF(pkg));
	if (verbosity > 1) printf("%s: ", pkg->name->name);
	apk_pkg_foreach_reverse_dependency(
		pkg,
		APK_FOREACH_INSTALLED | APK_FOREACH_NO_CONFLICTS | APK_DEP_SATISFIES | apk_foreach_genid(),
		print_rdep_pkg, NULL);
	puts("");
}

static void info_print_rinstall_if(struct apk_database *db, struct apk_package *pkg)
{
	char *separator = verbosity > 1 ? " " : "\n";

	if (verbosity == 1) printf(PKG_VER_FMT " affects auto-installation of:\n", PKG_VER_PRINTF(pkg));
	if (verbosity > 1) printf("%s: ", pkg->name->name);

	apk_array_foreach_item(name0, pkg->name->rinstall_if) {
		/* Check only the package that is installed, and that
		 * it actually has this package in install_if. */
		struct apk_package *pkg0 = apk_pkg_get_installed(name0);
		if (pkg0 == NULL) continue;
		apk_array_foreach(dep, pkg0->install_if) {
			if (dep->name != pkg->name) continue;
			if (apk_dep_conflict(dep)) continue;
			printf(PKG_VER_FMT "%s", PKG_VER_PRINTF(pkg0), separator);
			break;
		}
	}
	puts("");
}

static void info_print_contents(struct apk_database *db, struct apk_package *pkg)
{
	struct apk_installed_package *ipkg = pkg->ipkg;

	if (verbosity == 1) printf(PKG_VER_FMT " contains:\n", PKG_VER_PRINTF(pkg));

	apk_array_foreach_item(diri, ipkg->diris) {
		apk_array_foreach_item(file, diri->files) {
			if (verbosity > 1) printf("%s: ", pkg->name->name);
			printf(DIR_FILE_FMT "\n", DIR_FILE_PRINTF(diri->dir, file));
		}
	}
	puts("");
}

static void info_print_triggers(struct apk_database *db, struct apk_package *pkg)
{
	struct apk_installed_package *ipkg = pkg->ipkg;

	if (verbosity == 1) printf(PKG_VER_FMT " triggers:\n", PKG_VER_PRINTF(pkg));
	apk_array_foreach_item(trigger, ipkg->triggers) {
		if (verbosity > 1)
			printf("%s: trigger ", pkg->name->name);
		printf("%s\n", trigger);
	}
	puts("");
}

static void info_subactions(struct info_ctx *ctx, struct apk_package *pkg)
{
	struct apk_database *db = ctx->db;
	uint64_t fields = db->ctx->query.fields;
	if (!pkg->ipkg) {
		// info applet prints reverse dependencies only for installed packages
		const uint64_t ipkg_fields = APK_Q_FIELDS_ONLY_IPKG |
			BIT(APK_Q_FIELD_REV_DEPENDS) |
			BIT(APK_Q_FIELD_REV_INSTALL_IF);
		if (fields & ipkg_fields) {
			ctx->partial_result = 1;
			fields &= ~ipkg_fields;
		}
	}
	if (fields & BIT(APK_Q_FIELD_DESCRIPTION)) info_print_blob(db, pkg, "description", *pkg->description);
	if (fields & BIT(APK_Q_FIELD_URL)) info_print_blob(db, pkg, "webpage", *pkg->url);
	if (fields & BIT(APK_Q_FIELD_INSTALLED_SIZE)) info_print_size(db, pkg);
	if (fields & BIT(APK_Q_FIELD_DEPENDS)) info_print_dep_array(db, pkg, pkg->depends, "depends on");
	if (fields & BIT(APK_Q_FIELD_PROVIDES)) info_print_dep_array(db, pkg, pkg->provides, "provides");
	if (fields & BIT(APK_Q_FIELD_REV_DEPENDS)) info_print_required_by(db, pkg);
	if (fields & BIT(APK_Q_FIELD_CONTENTS)) info_print_contents(db, pkg);
	if (fields & BIT(APK_Q_FIELD_TRIGGERS)) info_print_triggers(db, pkg);
	if (fields & BIT(APK_Q_FIELD_INSTALL_IF)) info_print_dep_array(db, pkg, pkg->install_if, "has auto-install rule");
	if (fields & BIT(APK_Q_FIELD_REV_INSTALL_IF)) info_print_rinstall_if(db, pkg);
	if (fields & BIT(APK_Q_FIELD_REPLACES)) info_print_dep_array(db, pkg, pkg->ipkg->replaces, "replaces");
	if (fields & BIT(APK_Q_FIELD_LICENSE)) info_print_blob(db, pkg, "license", *pkg->license);
}

#define INFO_OPTIONS(OPT) \
	OPT(OPT_INFO_all,		APK_OPT_SH("a") "all") \
	OPT(OPT_INFO_contents,		APK_OPT_SH("L") "contents") \
	OPT(OPT_INFO_depends,		APK_OPT_SH("R") "depends") \
	OPT(OPT_INFO_description,	APK_OPT_SH("d") "description") \
	OPT(OPT_INFO_exists,		APK_OPT_SH("e") "exists") \
	OPT(OPT_INFO_install_if,	"install-if") \
	OPT(OPT_INFO_installed,		"installed") \
	OPT(OPT_INFO_license,		"license") \
	OPT(OPT_INFO_provides,		APK_OPT_SH("P") "provides") \
	OPT(OPT_INFO_rdepends,		APK_OPT_SH("r") "rdepends") \
	OPT(OPT_INFO_replaces,		"replaces") \
	OPT(OPT_INFO_rinstall_if,	"rinstall-if") \
	OPT(OPT_INFO_size,		APK_OPT_SH("s") "size") \
	OPT(OPT_INFO_triggers,		APK_OPT_SH("t") "triggers") \
	OPT(OPT_INFO_webpage,		APK_OPT_SH("w") "webpage") \
	OPT(OPT_INFO_who_owns,		APK_OPT_SH("W") "who-owns")

APK_OPTIONS(info_options_desc, INFO_OPTIONS);

static int info_parse_option(void *pctx, struct apk_ctx *ac, int opt, const char *optarg)
{
	struct info_ctx *ctx = (struct info_ctx *) pctx;
	struct apk_query_spec *qs = &ac->query;

	ctx->who_owns = ctx->exists_test = 0;
	switch (opt) {
	case OPT_INFO_exists:
	case OPT_INFO_installed:
		ctx->exists_test = 1;
		ac->open_flags |= APK_OPENF_NO_REPOS;
		break;
	case OPT_INFO_who_owns:
		ctx->who_owns = 1;
		ac->open_flags |= APK_OPENF_NO_REPOS;
		break;
	case OPT_INFO_webpage:
		qs->fields |= BIT(APK_Q_FIELD_URL);
		break;
	case OPT_INFO_depends:
		qs->fields |= BIT(APK_Q_FIELD_DEPENDS);
		break;
	case OPT_INFO_provides:
		qs->fields |= BIT(APK_Q_FIELD_PROVIDES);
		break;
	case OPT_INFO_rdepends:
		qs->fields |= BIT(APK_Q_FIELD_REV_DEPENDS);
		break;
	case OPT_INFO_install_if:
		qs->fields |= BIT(APK_Q_FIELD_INSTALL_IF);
		break;
	case OPT_INFO_rinstall_if:
		qs->fields |= BIT(APK_Q_FIELD_REV_INSTALL_IF);
		break;
	case OPT_INFO_size:
		qs->fields |= BIT(APK_Q_FIELD_INSTALLED_SIZE);
		break;
	case OPT_INFO_description:
		qs->fields |= BIT(APK_Q_FIELD_DESCRIPTION);
		break;
	case OPT_INFO_contents:
		qs->fields |= BIT(APK_Q_FIELD_CONTENTS);
		break;
	case OPT_INFO_triggers:
		qs->fields |= BIT(APK_Q_FIELD_TRIGGERS);
		break;
	case OPT_INFO_replaces:
		qs->fields |= BIT(APK_Q_FIELD_REPLACES);
		break;
	case OPT_INFO_license:
		qs->fields |= BIT(APK_Q_FIELD_LICENSE);
		break;
	case OPT_INFO_all:
		qs->fields |= APK_Q_FIELDS_ALL;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static int info_main(void *ctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_out *out = &ac->out;
	struct apk_database *db = ac->db;
	struct apk_query_spec *qs = &ac->query;
	struct info_ctx *ictx = (struct info_ctx *) ctx;
	struct apk_package_array *pkgs;
	int oneline = 0;

	verbosity = apk_out_verbosity(out);
	ictx->db = db;
	qs->filter.revdeps_installed = 1;
	qs->revdeps_field = APK_Q_FIELD_PACKAGE;

	if (ictx->who_owns) return info_who_owns(ctx, db, args);
	if (ictx->exists_test) return info_exists(ctx, db, args);

	qs->filter.all_matches = 1;
	if (apk_array_len(args) == 0) {
		qs->filter.installed = 1;
		qs->mode.empty_matches_all = 1;
		oneline = 1;
	}
	if (!qs->fields) qs->fields = BIT(APK_Q_FIELD_DESCRIPTION) | BIT(APK_Q_FIELD_URL) |
		BIT(APK_Q_FIELD_INSTALLED_SIZE);
	qs->fields |= BIT(APK_Q_FIELD_NAME) | BIT(APK_Q_FIELD_VERSION);
	if (!qs->match) qs->match = BIT(APK_Q_FIELD_NAME) | BIT(APK_Q_FIELD_PROVIDES);
	if (qs->ser == &apk_serializer_query && (oneline || ac->legacy_info)) {
		apk_package_array_init(&pkgs);
		int errors = apk_query_packages(ac, qs, args, &pkgs);
		if (oneline) {
			apk_array_foreach_item(pkg, pkgs) info_print_pkg_oneline(pkg, 1);
		}else {
			apk_array_foreach_item(pkg, pkgs) info_subactions(ctx, pkg);
		}
		apk_package_array_free(&pkgs);
		if (errors == 0 && ictx->partial_result && qs->fields == APK_Q_FIELDS_ALL)
			return 1;
		return errors;
	}
	return apk_query_main(ac, args);
}

static struct apk_applet apk_info = {
	.name = "info",
	.options_desc = info_options_desc,
	.optgroup_query = 1,
	.open_flags = APK_OPENF_READ | APK_OPENF_ALLOW_ARCH,
	.context_size = sizeof(struct info_ctx),
	.parse = info_parse_option,
	.main = info_main,
};

APK_DEFINE_APPLET(apk_info);
