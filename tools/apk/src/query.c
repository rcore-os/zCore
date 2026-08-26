/* query.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2025 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <unistd.h>
#include <fnmatch.h>
#include "apk_blob.h"
#include "apk_database.h"
#include "apk_package.h"
#include "apk_solver.h"
#include "apk_query.h"
#include "apk_applet.h"
#include "apk_pathbuilder.h"

// id, key, printable
#define DECLARE_FIELDS(func) \
	func(APK_Q_FIELD_QUERY,		"query",		"Query") \
	func(APK_Q_FIELD_ERROR,		"error",		"Error") \
	func(APK_Q_FIELD_PATH_TARGET,	"path-target",		"Path-Target") \
	func(APK_Q_FIELD_OWNER,		"owner",		"Owner") \
	\
	func(APK_Q_FIELD_PACKAGE,	"package",		"Package") \
	func(APK_Q_FIELD_NAME,		"name",			"Name") \
	func(APK_Q_FIELD_VERSION,	"version",		"Version") \
	func(APK_Q_FIELD_HASH,		"hash",			"Hash") \
	func(APK_Q_FIELD_DESCRIPTION,	"description",		"Description") \
	func(APK_Q_FIELD_ARCH,		"arch",			"Arch") \
	func(APK_Q_FIELD_LICENSE,	"license",		"License") \
	func(APK_Q_FIELD_ORIGIN,	"origin",		"Origin") \
	func(APK_Q_FIELD_MAINTAINER,	"maintainer",		"Maintainer") \
	func(APK_Q_FIELD_URL,		"url",			"URL") \
	func(APK_Q_FIELD_COMMIT,	"commit",		"Commit") \
	func(APK_Q_FIELD_BUILD_TIME,	"build-time",		"Build-Time") \
	func(APK_Q_FIELD_INSTALLED_SIZE,"installed-size",	"Installed-Size") \
	func(APK_Q_FIELD_FILE_SIZE,	"file-size",		"File-Size") \
	func(APK_Q_FIELD_PROVIDER_PRIORITY,"provider-priority", "Provider-Priority") \
	func(APK_Q_FIELD_DEPENDS,	"depends",		"Depends") \
	func(APK_Q_FIELD_PROVIDES,	"provides",		"Provides") \
	func(APK_Q_FIELD_REPLACES,	"replaces",		"Replaces") \
	func(APK_Q_FIELD_INSTALL_IF,	"install-if",		"Install-If") \
	func(APK_Q_FIELD_RECOMMENDS,	"recommends",		"Recommends") \
	func(APK_Q_FIELD_LAYER,		"layer",		"Layer") \
	func(APK_Q_FIELD_TAGS,		"tags",			"Tags") \
	\
	func(APK_Q_FIELD_CONTENTS,	"contents",		"Contents") \
	func(APK_Q_FIELD_TRIGGERS,	"triggers",		"Triggers") \
	func(APK_Q_FIELD_SCRIPTS,	"scripts",		"Scripts") \
	func(APK_Q_FIELD_REPLACES_PRIORITY,"replaces-priority", "Replaces-Priority") \
	func(APK_Q_FIELD_STATUS,	"status",		"Status") \
	\
	func(APK_Q_FIELD_REPOSITORIES,	"repositories",		"Repositories") \
	func(APK_Q_FIELD_DOWNLOAD_URL,	"download-url",		"Download-URL") \
	\
	func(APK_Q_FIELD_REV_DEPENDS,	"reverse-depends",	"Reverse-Depends") \
	func(APK_Q_FIELD_REV_INSTALL_IF,"reverse-install-if",	"Reverse-Install-If") \


#define FIELD_DEFINE(n, key, str) char field__##n[sizeof(str)];
#define FIELD_ASSIGN_KEY(n, key, str) key,
#define FIELD_ASSIGN_STR(n, key, str) str,
static const struct field_mapping {
	DECLARE_FIELDS(FIELD_DEFINE)
} field_keys = {
	DECLARE_FIELDS(FIELD_ASSIGN_KEY)
}, field_strs = {
	DECLARE_FIELDS(FIELD_ASSIGN_STR)
};

#define FIELD_INDEX(n, key, str) [n] = offsetof(struct field_mapping, field__##n),
static const unsigned short field_index[] = {
	DECLARE_FIELDS(FIELD_INDEX)
	sizeof(struct field_mapping)
};

#define APK_Q_FIELD_SUMMARIZE \
	(BIT(APK_Q_FIELD_PACKAGE) | BIT(APK_Q_FIELD_NAME) | BIT(APK_Q_FIELD_ORIGIN) |\
	 BIT(APK_Q_FIELD_DEPENDS) | BIT(APK_Q_FIELD_PROVIDES) | BIT(APK_Q_FIELD_REPLACES) | \
	 BIT(APK_Q_FIELD_INSTALL_IF) | BIT(APK_Q_FIELD_RECOMMENDS) | \
	 BIT(APK_Q_FIELD_REV_DEPENDS) | BIT(APK_Q_FIELD_REV_INSTALL_IF))

static int popcount(uint64_t val)
{
#if __has_builtin(__builtin_popcountll)
	return __builtin_popcountll(val);
#else
	int count = 0;
	for (int i = 0; i < 64; i++)
		if (val & BIT(i)) count++;
	return count;
#endif
}

static const char *field_key(int f)
{
	return (const char*)&field_keys + field_index[f];
}

int apk_query_field_by_name(apk_blob_t k)
{
	void *prev = (void*) field_key(0), *ptr;
	for (int i = 1; i < ARRAY_SIZE(field_index); i++, prev = ptr) {
		ptr = (void*) field_key(i);
		if (apk_blob_compare(APK_BLOB_PTR_PTR(prev, ptr-2), k) == 0)
			return i - 1;
	}
	return -1;
}

uint64_t apk_query_fields(apk_blob_t field_list, uint64_t allowed_fields)
{
	uint64_t fields = 0;

	if (apk_blob_compare(field_list, APK_BLOB_STRLIT("all")) == 0)
		return APK_Q_FIELDS_ALL;

	apk_blob_foreach_token(word, field_list, APK_BLOB_STRLIT(",")) {
		int f = apk_query_field_by_name(word);
		if (f < 0 || !(BIT(f) & allowed_fields)) return 0;
		fields |= BIT(f);
	}
	return fields;
}

apk_blob_t apk_query_field(int f)
{
	return APK_BLOB_PTR_PTR((void*)field_key(f), (void*)(field_key(f+1)-2));
}

apk_blob_t apk_query_printable_field(apk_blob_t f)
{
	if (f.ptr >= (const char*)&field_keys && f.ptr < (const char*)&field_keys + sizeof field_keys)
		return APK_BLOB_PTR_LEN((char*)f.ptr - (char*)&field_keys + (char*)&field_strs, f.len);
	return f;
}

#define QUERY_OPTIONS(OPT) \
	OPT(OPT_QUERY_all_matches,	"all-matches") \
	OPT(OPT_QUERY_available,	"available") \
	OPT(OPT_QUERY_fields,		APK_OPT_ARG APK_OPT_SH("F") "fields") \
	OPT(OPT_QUERY_format,		APK_OPT_ARG "format") \
	OPT(OPT_QUERY_from,		APK_OPT_ARG "from") \
	OPT(OPT_QUERY_installed,	"installed") \
	OPT(OPT_QUERY_match,		APK_OPT_ARG "match") \
	OPT(OPT_QUERY_recursive,	APK_OPT_SH("R") "recursive") \
	OPT(OPT_QUERY_search,		"search") \
	OPT(OPT_QUERY_summarize,	APK_OPT_ARG "summarize") \
	OPT(OPT_QUERY_upgradable,	"upgradable") \
	OPT(OPT_QUERY_world,		"world") \
	OPT(OPT_QUERY_orphaned,		"orphaned") \

APK_OPTIONS_EXT(optgroup_query_desc, QUERY_OPTIONS);

static int parse_fields_and_revfield(apk_blob_t b, uint64_t allowed_fields, uint64_t *fields, uint8_t *revfield)
{
	apk_blob_t rev;
	int f;

	if (apk_blob_split(b, APK_BLOB_STRLIT(":"), &b, &rev)) {
		f = apk_query_field_by_name(rev);
		if (f < 0 || (BIT(f) & (BIT(APK_Q_FIELD_NAME)|BIT(APK_Q_FIELD_PACKAGE)|BIT(APK_Q_FIELD_ORIGIN))) == 0)
			return -EINVAL;
		*revfield = f;
	}
	*fields = apk_query_fields(b, allowed_fields);
	if (!*fields) return -EINVAL;
	return 0;
}

int apk_query_parse_option(struct apk_ctx *ac, int opt, const char *optarg)
{
	const unsigned long all_flags = APK_OPENF_NO_SYS_REPOS | APK_OPENF_NO_INSTALLED_REPO | APK_OPENF_NO_INSTALLED;
	struct apk_query_spec *qs = &ac->query;
	unsigned long flags;

	switch (opt) {
	case OPT_QUERY_all_matches:
		qs->filter.all_matches = 1;
		break;
	case OPT_QUERY_available:
		qs->filter.available = 1;
		break;
	case OPT_QUERY_fields:
		qs->mode.summarize = 0;
		return parse_fields_and_revfield(APK_BLOB_STR(optarg), APK_Q_FIELDS_ALL, &qs->fields, &qs->revdeps_field);
	case OPT_QUERY_format:
		qs->ser = apk_serializer_lookup(optarg, &apk_serializer_query);
		if (IS_ERR(qs->ser)) return -EINVAL;
		break;
	case OPT_QUERY_installed:
		qs->filter.installed = 1;
		// implies --from installed
		ac->open_flags &= ~all_flags;
		ac->open_flags |= APK_OPENF_NO_SYS_REPOS;
		break;
	case OPT_QUERY_match:
		qs->match = apk_query_fields(APK_BLOB_STR(optarg), APK_Q_FIELDS_MATCHABLE);
		if (!qs->match) return -EINVAL;
		break;
	case OPT_QUERY_recursive:
		qs->mode.recursive = 1;
		break;
	case OPT_QUERY_search:
		qs->mode.search = 1;
		break;
	case OPT_QUERY_summarize:
		qs->mode.summarize = 1;
		if (parse_fields_and_revfield(APK_BLOB_STR(optarg), APK_Q_FIELD_SUMMARIZE, &qs->fields, &qs->revdeps_field) < 0)
			return -EINVAL;
		if (popcount(qs->fields) != 1) return -EINVAL;
		return 0;
	case OPT_QUERY_upgradable:
		qs->filter.upgradable = 1;
		break;
	case OPT_QUERY_world:
		qs->mode.recursive = 1;
		qs->mode.world = 1;
		ac->open_flags &= ~APK_OPENF_NO_WORLD;
		break;
	case OPT_QUERY_from:
		if (strcmp(optarg, "none") == 0) {
			flags = APK_OPENF_NO_SYS_REPOS | APK_OPENF_NO_INSTALLED_REPO | APK_OPENF_NO_INSTALLED;
		} else if (strcmp(optarg, "repositories") == 0) {
			flags = APK_OPENF_NO_INSTALLED_REPO | APK_OPENF_NO_INSTALLED;
		} else if (strcmp(optarg, "installed") == 0) {
			flags = APK_OPENF_NO_SYS_REPOS;
		} else if (strcmp(optarg, "system") == 0) {
			flags = 0;
		} else
			return -EINVAL;

		ac->open_flags &= ~all_flags;
		ac->open_flags |= flags;
		break;
	case OPT_QUERY_orphaned:
		qs->filter.orphaned = 1;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static int serialize_blobptr_array(struct apk_serializer *ser, struct apk_blobptr_array *a)
{
	apk_ser_start_array(ser, apk_array_len(a));
	apk_array_foreach_item(item, a) apk_ser_string(ser, *item);
	return apk_ser_end(ser);
}

static int num_scripts(const struct apk_installed_package *ipkg)
{
	int num = 0;
	for (int i = 0; i < ARRAY_SIZE(ipkg->script); i++) if (ipkg->script[i].len) num++;
	return num;
}

struct pkgser_ops;
typedef void (*revdep_serializer_f)(struct apk_package *pkg0, struct apk_dependency *dep0, struct apk_package *pkg, void *ctx);

struct pkgser_ctx {
	struct apk_database *db;
	struct apk_serializer *ser;
	struct apk_query_spec *qs;
	const struct pkgser_ops *ops;
	revdep_serializer_f revdep_serializer;
};

struct pkgser_ops {
	void (*name)(struct pkgser_ctx*, struct apk_name *name);
	void (*package)(struct pkgser_ctx*, struct apk_package *name);
	void (*dependencies)(struct pkgser_ctx*, struct apk_dependency_array *deps, bool provides);
};

/* Reverse dependency target field serialzer */
static void serialize_revdep_unique_name(struct pkgser_ctx *pc, struct apk_name *name, unsigned int genid)
{
	if (name->foreach_genid >= genid) return;
	name->foreach_genid = genid;
	pc->ops->name(pc, name);
}

static void serialize_revdep_name(struct apk_package *pkg0, struct apk_dependency *dep0, struct apk_package *pkg, void *ctx)
{
	serialize_revdep_unique_name(ctx, pkg0->name, pkg0->foreach_genid);
}

static void serialize_revdep_package(struct apk_package *pkg0, struct apk_dependency *dep0, struct apk_package *pkg, void *ctx)
{
	struct pkgser_ctx *pc = ctx;
	pc->ops->package(pc, pkg0);
}

static void serialize_revdep_origin(struct apk_package *pkg0, struct apk_dependency *dep0, struct apk_package *pkg, void *ctx)
{
	struct pkgser_ctx *pc = ctx;
	if (pkg->origin->len) serialize_revdep_unique_name(pc, apk_db_get_name(pc->db, *pkg0->origin), pkg0->foreach_genid);
}

static revdep_serializer_f revdep_serializer(uint8_t rev_field)
{
	switch (rev_field) {
	case APK_Q_FIELD_PACKAGE:
		return &serialize_revdep_package;
	case APK_Q_FIELD_ORIGIN:
		return &serialize_revdep_origin;
	case APK_Q_FIELD_NAME:
	default:
		return &serialize_revdep_name;
	}
}

/* Output directly to serializer */
static void pkgser_serialize_name(struct pkgser_ctx *pc, struct apk_name *name)
{
	apk_ser_string(pc->ser, APK_BLOB_STR(name->name));
}

static void pkgser_serialize_package(struct pkgser_ctx *pc, struct apk_package *pkg)
{
	char buf[PKG_VER_MAX];
	apk_ser_string(pc->ser, apk_blob_fmt(buf, sizeof buf, PKG_VER_FMT, PKG_VER_PRINTF(pkg)));
}

static void pkgser_serialize_dependencies(struct pkgser_ctx *pc, struct apk_dependency_array *deps, bool provides)
{
	char buf[1024];
	apk_ser_start_array(pc->ser, apk_array_len(deps));
	apk_array_foreach(dep, deps)
		apk_ser_string(pc->ser, apk_blob_fmt(buf, sizeof buf, DEP_FMT, DEP_PRINTF(dep)));
	apk_ser_end(pc->ser);
}

static const struct pkgser_ops pkgser_serialize = {
	.name = pkgser_serialize_name,
	.package = pkgser_serialize_package,
	.dependencies = pkgser_serialize_dependencies,
};

/* FIELDS_SERIALIZE* require 'ser' and 'fields' defined on scope */
#define FIELD_SERIALIZE(_f, _action)				\
	do { if ((fields & BIT(_f))) {				\
		apk_ser_key(ser, apk_query_field(_f));		\
		_action;					\
	} } while (0)

#define FIELD_SERIALIZE_BLOB(_f, _val)			if ((_val).len) FIELD_SERIALIZE(_f, apk_ser_string(ser, _val));
#define FIELD_SERIALIZE_NUMERIC(_f, _val)		if (_val) FIELD_SERIALIZE(_f, apk_ser_numeric(ser, _val, 0));
#define FIELD_SERIALIZE_ARRAY(_f, _val, _action) 	if (apk_array_len(_val)) FIELD_SERIALIZE(_f, _action);

static int __apk_package_serialize(struct apk_package *pkg, struct pkgser_ctx *pc)
{
	struct apk_database *db = pc->db;
	struct apk_serializer *ser = pc->ser;
	struct apk_query_spec *qs = pc->qs;
	uint64_t fields = qs->fields;
	unsigned int revdeps_installed = qs->filter.revdeps_installed ? APK_FOREACH_INSTALLED : 0;
	int ret = 0;

	FIELD_SERIALIZE(APK_Q_FIELD_PACKAGE, pc->ops->package(pc, pkg));
	FIELD_SERIALIZE(APK_Q_FIELD_NAME, pc->ops->name(pc, pkg->name));
	FIELD_SERIALIZE_BLOB(APK_Q_FIELD_VERSION, *pkg->version);
	//APK_Q_FIELD_HASH
	if (fields & BIT(APK_Q_FIELD_HASH)) ret = 1;
	FIELD_SERIALIZE_BLOB(APK_Q_FIELD_DESCRIPTION, *pkg->description);
	FIELD_SERIALIZE_BLOB(APK_Q_FIELD_ARCH, *pkg->arch);
	FIELD_SERIALIZE_BLOB(APK_Q_FIELD_LICENSE, *pkg->license);
	FIELD_SERIALIZE_BLOB(APK_Q_FIELD_ORIGIN, *pkg->origin);
	FIELD_SERIALIZE_BLOB(APK_Q_FIELD_MAINTAINER, *pkg->maintainer);
	FIELD_SERIALIZE_BLOB(APK_Q_FIELD_URL, *pkg->url);
	FIELD_SERIALIZE_BLOB(APK_Q_FIELD_COMMIT, *pkg->commit);
	FIELD_SERIALIZE_NUMERIC(APK_Q_FIELD_BUILD_TIME, pkg->build_time);
	FIELD_SERIALIZE_NUMERIC(APK_Q_FIELD_INSTALLED_SIZE, pkg->installed_size);
	FIELD_SERIALIZE_NUMERIC(APK_Q_FIELD_FILE_SIZE, pkg->size);
	FIELD_SERIALIZE_NUMERIC(APK_Q_FIELD_PROVIDER_PRIORITY, pkg->provider_priority);
	FIELD_SERIALIZE_ARRAY(APK_Q_FIELD_DEPENDS, pkg->depends, pc->ops->dependencies(pc, pkg->depends, false));
	FIELD_SERIALIZE_ARRAY(APK_Q_FIELD_PROVIDES, pkg->provides, pc->ops->dependencies(pc, pkg->provides, true));
	FIELD_SERIALIZE_ARRAY(APK_Q_FIELD_INSTALL_IF, pkg->install_if, pc->ops->dependencies(pc, pkg->install_if, false));
	FIELD_SERIALIZE_ARRAY(APK_Q_FIELD_RECOMMENDS, pkg->recommends, pc->ops->dependencies(pc, pkg->recommends, false));
	FIELD_SERIALIZE_NUMERIC(APK_Q_FIELD_LAYER, pkg->layer);
	FIELD_SERIALIZE_ARRAY(APK_Q_FIELD_TAGS, pkg->tags, serialize_blobptr_array(ser, pkg->tags));

	// synthetic/repositories fields
	if (BIT(APK_Q_FIELD_REPOSITORIES) & fields) {
		char buf[NAME_MAX];
		apk_ser_key(ser, apk_query_field(APK_Q_FIELD_REPOSITORIES));
		apk_ser_start_array(ser, -1);
		if (pkg->ipkg) apk_ser_string(ser, apk_blob_fmt(buf, sizeof buf, "%s/installed", apk_db_layer_name(pkg->layer)));
		for (int i = 0; i < db->num_repos; i++) {
			if (!(BIT(i) & pkg->repos)) continue;
			apk_ser_string(ser, db->repos[i].url_printable);
		}
		apk_ser_end(ser);
	}
	if (BIT(APK_Q_FIELD_DOWNLOAD_URL) & fields) {
		char buf[PATH_MAX];
		struct apk_repository *repo = apk_db_select_repo(db, pkg);
		if (repo && apk_repo_package_url(db, repo, pkg, NULL, buf, sizeof buf) == 0) {
			apk_ser_key(ser, apk_query_field(APK_Q_FIELD_DOWNLOAD_URL));
			apk_ser_string(ser, APK_BLOB_STR(buf));
		}
	}

	if (BIT(APK_Q_FIELD_REV_DEPENDS) & fields) {
		apk_ser_key(ser, apk_query_field(APK_Q_FIELD_REV_DEPENDS));
		apk_ser_start_array(ser, -1);
		apk_pkg_foreach_reverse_dependency(
			pkg, APK_DEP_SATISFIES | APK_FOREACH_NO_CONFLICTS | revdeps_installed | apk_foreach_genid(),
			pc->revdep_serializer, pc);
		apk_ser_end(ser);
	}
	if (BIT(APK_Q_FIELD_REV_INSTALL_IF) & fields) {
		unsigned int match = apk_foreach_genid();
		apk_ser_key(ser, apk_query_field(APK_Q_FIELD_REV_INSTALL_IF));
		apk_ser_start_array(ser, -1);
		apk_array_foreach_item(name0, pkg->name->rinstall_if) {
			apk_array_foreach(p, name0->providers) {
				apk_array_foreach(dep, p->pkg->install_if) {
					if (apk_dep_conflict(dep)) continue;
					if (revdeps_installed && !p->pkg->ipkg) continue;
					if (apk_dep_analyze(p->pkg, dep, pkg) & APK_DEP_SATISFIES) {
						if (apk_pkg_match_genid(p->pkg, match)) continue;
						pc->revdep_serializer(p->pkg, dep, pkg, pc);
					}
				}
			}
		}
		apk_ser_end(ser);
	}

	if (!pkg->ipkg) {
		if (fields & APK_Q_FIELDS_ONLY_IPKG) ret = 1;
		return ret;
	}

	// installed package fields
	struct apk_installed_package *ipkg = pkg->ipkg;
	if (BIT(APK_Q_FIELD_CONTENTS) & fields) {
		struct apk_pathbuilder pb;

		apk_ser_key(ser, apk_query_field(APK_Q_FIELD_CONTENTS));
		apk_ser_start_array(ser, -1);
		apk_array_foreach_item(diri, ipkg->diris) {
			apk_pathbuilder_setb(&pb, APK_BLOB_PTR_LEN(diri->dir->name, diri->dir->namelen));
			apk_array_foreach_item(file, diri->files) {
				int n = apk_pathbuilder_pushb(&pb, APK_BLOB_PTR_LEN(file->name, file->namelen));
				apk_ser_string(ser, apk_pathbuilder_get(&pb));
				apk_pathbuilder_pop(&pb, n);
			}
		}
		apk_ser_end(ser);
	}
	if ((BIT(APK_Q_FIELD_TRIGGERS) & fields) && apk_array_len(ipkg->triggers)) {
		apk_ser_key(ser, apk_query_field(APK_Q_FIELD_TRIGGERS));
		apk_ser_start_array(ser, apk_array_len(ipkg->triggers));
		apk_array_foreach_item(str, ipkg->triggers)
			apk_ser_string(ser, APK_BLOB_STR(str));
		apk_ser_end(ser);
	}
	if ((BIT(APK_Q_FIELD_SCRIPTS) & fields) && num_scripts(ipkg)) {
		apk_ser_key(ser, apk_query_field(APK_Q_FIELD_SCRIPTS));
		apk_ser_start_array(ser, num_scripts(ipkg));
		for (int i = 0; i < ARRAY_SIZE(ipkg->script); i++) {
			if (!ipkg->script[i].len) continue;
			apk_ser_string(ser, APK_BLOB_STR(apk_script_types[i]));
		}
		apk_ser_end(ser);
	}

	FIELD_SERIALIZE_NUMERIC(APK_Q_FIELD_REPLACES_PRIORITY, ipkg->replaces_priority);
	FIELD_SERIALIZE_ARRAY(APK_Q_FIELD_REPLACES, ipkg->replaces, pc->ops->dependencies(pc, ipkg->replaces, false));
	if (BIT(APK_Q_FIELD_STATUS) & fields) {
		apk_ser_key(ser, apk_query_field(APK_Q_FIELD_STATUS));
		apk_ser_start_array(ser, -1);
		apk_ser_string(ser, APK_BLOB_STRLIT("installed"));
		if (ipkg->broken_files) apk_ser_string(ser, APK_BLOB_STRLIT("broken-files"));
		if (ipkg->broken_script) apk_ser_string(ser, APK_BLOB_STRLIT("broken-script"));
		if (ipkg->broken_xattr) apk_ser_string(ser, APK_BLOB_STRLIT("broken-xattr"));
		apk_ser_end(ser);
	}
	return ret;
}

int apk_package_serialize(struct apk_package *pkg, struct apk_database *db, struct apk_query_spec *qs, struct apk_serializer *ser)
{
	struct pkgser_ctx pc = {
		.db = db,
		.ser = ser,
		.qs = qs,
		.ops = &pkgser_serialize,
		.revdep_serializer = revdep_serializer(qs->revdeps_field),
	};
	return __apk_package_serialize(pkg, &pc);
}

int apk_query_match_serialize(struct apk_query_match *qm, struct apk_database *db, struct apk_query_spec *qs, struct apk_serializer *ser)
{
	uint64_t fields = qs->fields | BIT(APK_Q_FIELD_ERROR);

	FIELD_SERIALIZE_BLOB(APK_Q_FIELD_QUERY, qm->query);
	FIELD_SERIALIZE_BLOB(APK_Q_FIELD_PATH_TARGET, qm->path_target);

	if (qm->pkg) apk_package_serialize(qm->pkg, db, qs, ser);
	else FIELD_SERIALIZE_BLOB(APK_Q_FIELD_ERROR, APK_BLOB_STRLIT("owner not found"));

	return 0;
}

static struct apk_package *get_owner(struct apk_database *db, apk_blob_t fn)
{
	struct apk_db_dir *dir;

	apk_blob_pull_blob_match(&fn, APK_BLOB_STRLIT("/"));
	fn = apk_blob_trim_end(fn, '/');

	dir = apk_db_dir_query(db, fn);
	if (dir && dir->owner) return dir->owner->pkg;
	return apk_db_get_file_owner(db, fn);
}

static int apk_query_recursive(struct apk_ctx *ac, struct apk_query_spec *qs, struct apk_string_array *args, apk_query_match_cb match, void *pctx)
{
	struct apk_database *db = ac->db;
	struct apk_changeset changeset = {};
	struct apk_dependency_array *world;
	int r;

	apk_dependency_array_init(&world);
	apk_change_array_init(&changeset.changes);

	if (qs->mode.world)
		apk_dependency_array_copy(&world, db->world);

	apk_array_foreach_item(arg, args) {
		struct apk_dependency dep;
		apk_blob_t b = APK_BLOB_STR(arg);

		apk_blob_pull_dep(&b, ac->db, &dep, true);
		if (APK_BLOB_IS_NULL(b) || b.len > 0 || dep.broken) {
			apk_err(&ac->out, "'%s' is not a valid world dependency, format is name(@tag)([<>~=]version)",
				arg);
			r = -APKE_DEPENDENCY_FORMAT;
			goto err;
		}
		apk_dependency_array_add(&world, dep);
	}

	unsigned short flags = APK_SOLVERF_IGNORE_CONFLICT;
	if (qs->filter.available) flags |= APK_SOLVERF_AVAILABLE;

	r = apk_solver_solve(ac->db, flags, world, &changeset);
	if (r == 0) {
		apk_array_foreach(change, changeset.changes) {
			if (!change->new_pkg) continue;
			r = match(pctx, &(struct apk_query_match){ .pkg = change->new_pkg });
			if (r) break;
		}
	} else {
		apk_solver_print_errors(ac->db, &changeset, world);
	}

err:
	apk_change_array_free(&changeset.changes);
	apk_dependency_array_free(&world);
	return r;
}

int apk_query_who_owns(struct apk_database *db, const char *path, struct apk_query_match *qm, char *buf, size_t bufsz)
{
	apk_blob_t q = APK_BLOB_STR(path);
	*qm = (struct apk_query_match) {
		.query = q,
		.pkg = get_owner(db, q),
	};
	if (!qm->pkg) {
		ssize_t r = readlinkat(db->root_fd, path, buf, bufsz);
		if (r > 0 && r < PATH_MAX && buf[0] == '/') {
			qm->path_target = APK_BLOB_PTR_LEN(buf, r);
			qm->pkg = get_owner(db, qm->path_target);
			if (!qm->pkg) qm->path_target = APK_BLOB_NULL;
		}
	}
	return 0;
}

struct match_ctx {
	struct apk_database *db;
	struct apk_query_spec *qs;
	const char *match;
	apk_blob_t q;
	struct apk_dependency dep;
	struct apk_serializer ser;
	struct apk_package *best;
	int match_mode;
	apk_query_match_cb cb, ser_cb;
	void *cb_ctx, *ser_cb_ctx;
	bool has_matches, done_matching;
	struct apk_query_match qm;
};

enum {
	MATCH_EXACT,
	MATCH_WILDCARD
};

static bool match_string(struct match_ctx *ctx, const char *value)
{
	switch (ctx->match_mode) {
	case MATCH_EXACT:
		return strcmp(value, ctx->match) == 0;
	case MATCH_WILDCARD:
		return fnmatch(ctx->match, value, FNM_CASEFOLD) == 0;
	default:
		return false;
	}
}

static bool match_blob(struct match_ctx *ctx, apk_blob_t value)
{
	char buf[PATH_MAX];

	switch (ctx->match_mode) {
	case MATCH_EXACT:
		return apk_blob_compare(value, ctx->q) == 0;
	case MATCH_WILDCARD:
		return fnmatch(ctx->match, apk_fmts(buf, sizeof buf, BLOB_FMT, BLOB_PRINTF(value)), FNM_CASEFOLD) == 0;
	default:
		return false;
	}
}

static int ser_noop_start_array(struct apk_serializer *ser, int num)
{
	return 0;
}

static int ser_noop_end(struct apk_serializer *ser)
{
	return 0;
}

static int ser_noop_key(struct apk_serializer *ser, apk_blob_t key)
{
	return 0;
}

static int ser_match_string(struct apk_serializer *ser, apk_blob_t scalar, int multiline)
{
	struct match_ctx *m = container_of(ser, struct match_ctx, ser);
	if (m->done_matching || !match_blob(m, scalar)) return 0;
	m->cb(m->cb_ctx, &m->qm);
	m->has_matches = true;
	m->done_matching = !m->qs->filter.all_matches;
	return 0;
}

static void pkgpkgser_match_dependency(struct pkgser_ctx *pc, struct apk_dependency_array *deps, bool provides)
{
	struct apk_serializer *ser = pc->ser;
	struct match_ctx *m = container_of(ser, struct match_ctx, ser);
	if (m->done_matching) return;
	apk_array_foreach(dep, deps) {
		if (!match_blob(m, APK_BLOB_STR(dep->name->name))) continue;
		if (m->dep.op != APK_DEPMASK_ANY) {
			if (provides && !apk_version_match(*m->dep.version, m->dep.op, *dep->version)) continue;
			if (!provides && (m->dep.op != dep->op || apk_blob_compare(*m->dep.version, *dep->version))) continue;
		}
		m->qm.name = dep->name;
		m->cb(m->cb_ctx, &m->qm);
		m->has_matches = true;
		m->done_matching = !m->qs->filter.all_matches;
	}
	m->qm.name = NULL;
}

static struct apk_serializer_ops serialize_match = {
	.start_array = ser_noop_start_array,
	.end = ser_noop_end,
	.key = ser_noop_key,
	.string = ser_match_string,
};

static const struct pkgser_ops pkgser_match = {
	.name = pkgser_serialize_name,
	.package = pkgser_serialize_package,
	.dependencies = pkgpkgser_match_dependency,
};


static int update_best_match(void *pctx, struct apk_query_match *qm)
{
	struct match_ctx *m = pctx;

	if (m->best == qm->pkg) return 0;
	if (!m->best || qm->pkg->ipkg ||
	    apk_version_compare(*qm->pkg->version, *m->best->version) == APK_VERSION_GREATER)
		m->best = qm->pkg;
	return 0;
}

static int match_name(apk_hash_item item, void *pctx)
{
	struct match_ctx *m = pctx;
	struct apk_query_spec *qs = m->qs;
	struct apk_name *name = item;
	struct apk_query_spec qs_nonindex = { .fields = qs->match & ~BIT(APK_Q_FIELD_NAME) };
	struct pkgser_ctx pc = {
		.db = m->db,
		.ser = &m->ser,
		.qs = &qs_nonindex,
		.ops = &pkgser_match,
		.revdep_serializer = revdep_serializer(0),
	};
	bool name_match = false;
	int r = 0;

	// Simple filter: orphaned
	if (qs->filter.orphaned && name->has_repository_providers) return 0;
	if (qs->match & BIT(APK_Q_FIELD_NAME)) name_match = match_string(m, name->name);
	if (qs->match && !name_match && !qs_nonindex.fields) return 0;

	m->best = NULL;
	m->dep.name = name;
	apk_array_foreach(p, name->providers) {
		if (p->pkg->name != name) continue;
		// Simple filters: available, installed, upgradable
		if (qs->filter.installed && !p->pkg->ipkg) continue;
		if (qs->filter.available && !apk_db_pkg_available(m->db, p->pkg)) continue;
		if (qs->filter.upgradable && !apk_db_pkg_upgradable(m->db, p->pkg)) continue;

		m->qm.pkg = p->pkg;
		if (!qs->match || (name_match && apk_dep_is_provided(NULL, &m->dep, p))) {
			// Generic match without match term or name match
			m->has_matches = true;
			m->qm.name = name;
			r = m->cb(m->cb_ctx, &m->qm);
			if (r) return r;
			if (!qs->filter.all_matches) continue;
		}
		m->qm.name = NULL;
		m->done_matching = false;
		__apk_package_serialize(p->pkg, &pc);
	}
	if (m->best) {
		return m->ser_cb(m->ser_cb_ctx, &(struct apk_query_match) {
			.query = m->q,
			.pkg = m->best,
		});
	}
	return r;
}

int apk_query_matches(struct apk_ctx *ac, struct apk_query_spec *qs, struct apk_string_array *args, apk_query_match_cb match, void *pctx)
{
	char buf[PATH_MAX];
	struct apk_database *db = ac->db;
	struct match_ctx m = {
		.db = ac->db,
		.qs = qs,
		.cb = match,
		.cb_ctx = pctx,
		.ser_cb = match,
		.ser_cb_ctx = pctx,
		.ser.ops = &serialize_match,
	};
	int r, no_matches = 0;

	if (!qs->match) qs->match = BIT(APK_Q_FIELD_NAME);
	if (qs->match & ~APK_Q_FIELDS_MATCHABLE) return -ENOTSUP;

	if (qs->mode.empty_matches_all && apk_array_len(args) == 0) {
		qs->match = 0;
		return apk_hash_foreach(&db->available.names, match_name, &m);
	}
	if (qs->mode.recursive) return apk_query_recursive(ac, qs, args, match, pctx);

	// Instead of reporting all matches, report only best
	if (!qs->filter.all_matches) {
		m.cb = update_best_match;
		m.cb_ctx = &m;
	}

	apk_array_foreach_item(arg, args) {
		apk_blob_t bname, bvers;
		int op;

		m.has_matches = false;
		if ((qs->match & BIT(APK_Q_FIELD_OWNER)) && arg[0] == '/') {
			struct apk_query_match qm;
			apk_query_who_owns(db, arg, &qm, buf, sizeof buf);
			if (qm.pkg) {
				r = match(pctx, &qm);
				if (r) break;
				m.has_matches = true;
			}
		}

		if (qs->mode.search) {
			m.match_mode = MATCH_WILDCARD;
			m.q = apk_blob_fmt(buf, sizeof buf, "*%s*", arg);
			m.match = m.q.ptr;
			m.dep.op = APK_DEPMASK_ANY;
			m.dep.version = &apk_atom_null;
		} else {
			m.match_mode = strpbrk(arg, "?*") ? MATCH_WILDCARD : MATCH_EXACT;
			m.q = APK_BLOB_STR(arg);
			m.match = arg;

			if (apk_dep_parse(m.q, &bname, &op, &bvers) < 0)
				bname = m.q;

			m.q = bname;
			m.dep = (struct apk_dependency) {
				.version = apk_atomize_dup(&db->atoms, bvers),
				.op = op,
			};
		}

		if (qs->match == BIT(APK_Q_FIELD_NAME) && m.match_mode == MATCH_EXACT) {
			m.dep.name = apk_db_query_name(db, bname);
			if (m.dep.name) r = match_name(m.dep.name, &m);
		} else {
			// do full scan
			r = apk_hash_foreach(&db->available.names, match_name, &m);
			if (r) break;
		}
		if (!m.has_matches) {
			// report no match
			r = match(pctx, &(struct apk_query_match) { .query = m.q });
			if (r) break;
			if (m.match_mode == MATCH_EXACT) no_matches++;
		}
	}
	return no_matches;
}

static int select_package(void *pctx, struct apk_query_match *qm)
{
	struct apk_package_array **ppkgs = pctx;
	struct apk_package *pkg = qm->pkg;

	if (pkg && !pkg->seen) {
		pkg->seen = 1;
		apk_package_array_add(ppkgs, pkg);
	}
	return 0;
}

int apk_query_packages(struct apk_ctx *ac, struct apk_query_spec *qs, struct apk_string_array *args, struct apk_package_array **pkgs)
{
	int r;

	r = apk_query_matches(ac, qs, args, select_package, pkgs);
	if (r >= 0) apk_array_qsort(*pkgs, apk_package_array_qsort);
	apk_array_foreach_item(pkg, *pkgs) pkg->seen = 0;
	return r;
}

struct summary_ctx {
	struct pkgser_ctx pc;
	struct apk_serializer ser;
	struct apk_name_array *names;
	struct apk_package_array *pkgs;
};

static void pkgser_summarize_name(struct pkgser_ctx *pc, struct apk_name *name)
{
	struct summary_ctx *s = container_of(pc, struct summary_ctx, pc);
	if (name->state_int) return;
	apk_name_array_add(&s->names, name);
	name->state_int = 1;
}

static int ser_summarize_string(struct apk_serializer *ser, apk_blob_t scalar, int multiline)
{
	// currently can happen only for "--summarize origin"
	struct summary_ctx *s = container_of(ser, struct summary_ctx, ser);
	if (scalar.len) pkgser_summarize_name(&s->pc, apk_db_get_name(s->pc.db, scalar));
	return 0;
}

static void pkgser_summarize_package(struct pkgser_ctx *pc, struct apk_package *pkg)
{
	struct summary_ctx *s = container_of(pc, struct summary_ctx, pc);
	if (pkg->seen) return;
	apk_package_array_add(&s->pkgs, pkg);
	pkg->seen = 1;
}

static void pkgser_summarize_dependencies(struct pkgser_ctx *pc, struct apk_dependency_array *deps, bool provides)
{
	apk_array_foreach(dep, deps)
		if (!apk_dep_conflict(dep)) pkgser_summarize_name(pc, dep->name);
}

static struct apk_serializer_ops serialize_summarize = {
	.start_array = ser_noop_start_array,
	.end = ser_noop_end,
	.key = ser_noop_key,
	.string = ser_summarize_string,
};

static const struct pkgser_ops pkgser_summarize = {
	.name = pkgser_summarize_name,
	.package = pkgser_summarize_package,
	.dependencies = pkgser_summarize_dependencies,
};

static int summarize_package(void *pctx, struct apk_query_match *qm)
{
	if (!qm->pkg) return 0;
	return __apk_package_serialize(qm->pkg, pctx);
}

static int apk_query_summarize(struct apk_ctx *ac, struct apk_query_spec *qs, struct apk_string_array *args, struct apk_serializer *ser)
{
	struct summary_ctx s = {
		.pc = {
			.db = ac->db,
			.ser = &s.ser,
			.qs = qs,
			.ops = &pkgser_summarize,
			.revdep_serializer = revdep_serializer(qs->revdeps_field),
		},
		.ser = { .ops = &serialize_summarize },
	};

	if (popcount(qs->fields) != 1) return -EINVAL;

	apk_name_array_init(&s.names);
	apk_package_array_init(&s.pkgs);
	int r = apk_query_matches(ac, qs, args, summarize_package, &s.pc);
	if (apk_array_len(s.names)) {
		apk_array_qsort(s.names, apk_name_array_qsort);
		apk_ser_start_array(ser, apk_array_len(s.names));
		apk_array_foreach_item(name, s.names) {
			apk_ser_string(ser, APK_BLOB_STR(name->name));
			name->state_int = 0;
		}
		apk_ser_end(ser);
	} else if (apk_array_len(s.pkgs)) {
		char buf[PKG_VER_MAX];
		apk_array_qsort(s.pkgs, apk_package_array_qsort);
		apk_ser_start_array(ser, apk_array_len(s.pkgs));
		apk_array_foreach_item(pkg, s.pkgs) {
			apk_ser_string(ser, apk_blob_fmt(buf, sizeof buf, PKG_VER_FMT, PKG_VER_PRINTF(pkg)));
			pkg->seen = 0;
		}
		apk_ser_end(ser);
	}
	apk_name_array_free(&s.names);
	apk_package_array_free(&s.pkgs);
	return r;
}

int apk_query_run(struct apk_ctx *ac, struct apk_query_spec *qs, struct apk_string_array *args, struct apk_serializer *ser)
{
	struct apk_package_array *pkgs;
	int r;

	if (qs->mode.summarize) return apk_query_summarize(ac, qs, args, ser);
	if (!qs->fields) qs->fields = APK_Q_FIELDS_DEFAULT_PKG;

	// create list of packages that match
	apk_package_array_init(&pkgs);
	r = apk_query_packages(ac, qs, args, &pkgs);
	if (r < 0) goto ret;

	r = 0;
	apk_ser_start_array(ser, apk_array_len(pkgs));
	apk_array_foreach_item(pkg, pkgs) {
		apk_ser_start_object(ser);
		if (apk_package_serialize(pkg, ac->db, qs, ser) == 1) r = 1;
		apk_ser_end(ser);
	}
	apk_ser_end(ser);
	if (qs->fields == APK_Q_FIELDS_ALL) r = 0;
ret:
	apk_package_array_free(&pkgs);
	return r;
}

int apk_query_main(struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_serializer *ser;
	struct apk_query_spec *qs = &ac->query;
	struct apk_out *out = &ac->out;
	int r;

	ser = apk_serializer_init_alloca(ac, qs->ser, apk_ostream_to_fd(STDOUT_FILENO));
	if (IS_ERR(ser)) return PTR_ERR(ser);

	r = apk_query_run(ac, qs, args, ser);
	if (r < 0) apk_err(out, "query failed: %s", apk_error_str(r));
	apk_serializer_cleanup(ser);
	return r;
}
