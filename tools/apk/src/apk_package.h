/* apk_package.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include "apk_version.h"
#include "apk_hash.h"
#include "apk_io.h"
#include "apk_solver_data.h"

struct adb_obj;
struct apk_database;
struct apk_db_dir_instance_array;
struct apk_balloc;
struct apk_name;
struct apk_provider;
struct apk_trust;

#define APK_SCRIPT_INVALID		-1
#define APK_SCRIPT_PRE_INSTALL		0
#define APK_SCRIPT_POST_INSTALL		1
#define APK_SCRIPT_PRE_DEINSTALL	2
#define APK_SCRIPT_POST_DEINSTALL	3
#define APK_SCRIPT_PRE_UPGRADE		4
#define APK_SCRIPT_POST_UPGRADE		5
#define APK_SCRIPT_TRIGGER		6
#define APK_SCRIPT_MAX			7

#define APK_DEP_IRRELEVANT		0x01
#define APK_DEP_SATISFIES		0x02
#define APK_DEP_CONFLICTS		0x04
#define APK_FOREACH_NO_CONFLICTS	0x08
#define APK_FOREACH_INSTALLED		0x10
#define APK_FOREACH_MARKED		0x20
#define APK_FOREACH_NULL_MATCHES_ALL	0x40
#define APK_FOREACH_DEP			0x80
#define APK_FOREACH_GENID_MASK		0xffffff00

struct apk_dependency {
	struct apk_name *name;
	apk_blob_t *version;
	uint8_t op;
	uint16_t broken : 1;		// solver state
	uint16_t repository_tag : 6;	// world dependency only: tag
	uint16_t layer : 4;		// solver sets for 'world' dependencies only
};
APK_ARRAY(apk_dependency_array, struct apk_dependency);

struct apk_installed_package {
	struct apk_package *pkg;
	struct list_head installed_pkgs_list;
	struct list_head trigger_pkgs_list;
	struct apk_db_dir_instance_array *diris;
	apk_blob_t script[APK_SCRIPT_MAX];
	struct apk_string_array *triggers;
	struct apk_string_array *pending_triggers;
	struct apk_dependency_array *replaces;

	unsigned short replaces_priority;
	unsigned repository_tag : 6;
	unsigned run_all_triggers : 1;
	unsigned broken_files : 1;
	unsigned broken_script : 1;
	unsigned broken_xattr : 1;
	unsigned sha256_160 : 1;
	unsigned to_be_removed : 1;
};

struct apk_package {
	apk_hash_node hash_node;
	struct apk_name *name;
	struct apk_installed_package *ipkg;
	struct apk_dependency_array *depends, *install_if, *provides, *recommends;
	struct apk_blobptr_array *tags;
	apk_blob_t *version;
	apk_blob_t *arch, *license, *origin, *maintainer, *url, *description, *commit;
	uint64_t installed_size, size;
	time_t build_time;

	union {
		struct apk_solver_package_state ss;
		int state_int;
	};
	unsigned int foreach_genid;
	uint32_t repos;
	unsigned short provider_priority;
	unsigned short filename_ndx;

	unsigned char seen : 1;
	unsigned char marked : 1;
	unsigned char uninstallable : 1;
	unsigned char cached_non_repository : 1;
	unsigned char cached : 1;
	unsigned char layer : 3;
	uint8_t digest_alg;
	uint8_t digest[0];
};

static inline apk_blob_t apk_pkg_hash_blob(const struct apk_package *pkg) {
	return APK_BLOB_PTR_LEN((char*) pkg->digest, APK_DIGEST_LENGTH_SHA1);
}

static inline apk_blob_t apk_pkg_digest_blob(const struct apk_package *pkg) {
	return APK_BLOB_PTR_LEN((char*) pkg->digest, apk_digest_alg_len(pkg->digest_alg));
}

APK_ARRAY(apk_package_array, struct apk_package *);
int apk_package_array_qsort(const void *a, const void *b);

#define APK_PROVIDER_FROM_PACKAGE(pkg)	  (struct apk_provider){(pkg),(pkg)->version}
#define APK_PROVIDER_FROM_PROVIDES(pkg,p) (struct apk_provider){(pkg),(p)->version}

#define PKG_VER_MAX		256
#define PKG_VER_FMT		"%s-" BLOB_FMT
#define PKG_VER_PRINTF(pkg)	(pkg)->name->name, BLOB_PRINTF(*(pkg)->version)
#define PKG_VER_STRLEN(pkg)	(strlen(pkg->name->name) + 1 + pkg->version->len)

#define DEP_FMT			"%s%s%s" BLOB_FMT
#define DEP_PRINTF(dep)		apk_dep_conflict(dep) ? "!" : "", (dep)->name->name, \
				APK_BLOB_IS_NULL(*(dep)->version) ? "" : apk_version_op_string((dep)->op), \
				BLOB_PRINTF(*(dep)->version)

extern const char *apk_script_types[];

static inline int apk_dep_conflict(const struct apk_dependency *dep) { return !!(dep->op & APK_VERSION_CONFLICT); }
void apk_dep_from_pkg(struct apk_dependency *dep, struct apk_database *db,
		      struct apk_package *pkg);
int apk_dep_is_materialized(const struct apk_dependency *dep, const struct apk_package *pkg);
int apk_dep_is_provided(const struct apk_package *deppkg, const struct apk_dependency *dep, const struct apk_provider *p);
int apk_dep_analyze(const struct apk_package *deppkg, struct apk_dependency *dep, struct apk_package *pkg);

void apk_blob_push_dep(apk_blob_t *to, struct apk_database *, struct apk_dependency *dep);
void apk_blob_push_deps(apk_blob_t *to, struct apk_database *, struct apk_dependency_array *deps);
void apk_blob_pull_dep(apk_blob_t *from, struct apk_database *, struct apk_dependency *, bool);
int apk_blob_pull_deps(apk_blob_t *from, struct apk_database *, struct apk_dependency_array **, bool);

int apk_deps_write_layer(struct apk_database *db, struct apk_dependency_array *deps,
			 struct apk_ostream *os, apk_blob_t separator, unsigned layer);
int apk_deps_write(struct apk_database *db, struct apk_dependency_array *deps,
		   struct apk_ostream *os, apk_blob_t separator);

void apk_dep_from_adb(struct apk_dependency *dep, struct apk_database *db, struct adb_obj *d);
void apk_deps_from_adb(struct apk_dependency_array **deps, struct apk_database *db, struct adb_obj *da);

int apk_dep_parse(apk_blob_t spec, apk_blob_t *name, int *op, apk_blob_t *version);
void apk_deps_add(struct apk_dependency_array **deps, struct apk_dependency *dep);
void apk_deps_del(struct apk_dependency_array **deps, struct apk_name *name);
int apk_script_type(const char *name);

struct apk_package_tmpl {
	struct apk_database *db;
	struct apk_package pkg;
	struct apk_digest id;
};
void apk_pkgtmpl_init(struct apk_package_tmpl *tmpl, struct apk_database *db);
void apk_pkgtmpl_free(struct apk_package_tmpl *tmpl);
void apk_pkgtmpl_reset(struct apk_package_tmpl *tmpl);
int apk_pkgtmpl_add_info(struct apk_package_tmpl *tmpl, char field, apk_blob_t value);
void apk_pkgtmpl_from_adb(struct apk_package_tmpl *tmpl, struct adb_obj *pkginfo);

int apk_pkg_read(struct apk_database *db, const char *name, struct apk_package **pkg, int v3ok);
int apk_pkg_subst(void *ctx, apk_blob_t key, apk_blob_t *to);
int apk_pkg_subst_validate(apk_blob_t fmt);

struct apk_package *apk_pkg_get_installed(struct apk_name *name);
struct apk_installed_package *apk_pkg_install(struct apk_database *db, struct apk_package *pkg);
void apk_pkg_uninstall(struct apk_database *db, struct apk_package *pkg);

int apk_ipkg_assign_script(struct apk_installed_package *ipkg, unsigned int type, apk_blob_t blob);
int apk_ipkg_add_script(struct apk_installed_package *ipkg, struct apk_istream *is, unsigned int type, uint64_t size);
int apk_ipkg_run_script(struct apk_installed_package *ipkg, struct apk_database *db, unsigned int type, char **argv);

int apk_pkg_write_index_header(struct apk_package *pkg, struct apk_ostream *os);
int apk_pkg_write_index_entry(struct apk_package *pkg, struct apk_ostream *os);

int apk_pkg_version_compare(const struct apk_package *a, const struct apk_package *b);
int apk_pkg_cmp_display(const struct apk_package *a, const struct apk_package *b);

enum {
	APK_PKG_REPLACES_YES,
	APK_PKG_REPLACES_NO,
	APK_PKG_REPLACES_CONFLICT,
};
int apk_pkg_replaces_dir(const struct apk_package *a, const struct apk_package *b);
int apk_pkg_replaces_file(const struct apk_package *a, const struct apk_package *b);

unsigned int apk_foreach_genid(void);
int apk_pkg_match_genid(struct apk_package *pkg, unsigned int match);
void apk_pkg_foreach_matching_dependency(
		struct apk_package *pkg, struct apk_dependency_array *deps,
		unsigned int match, struct apk_package *mpkg,
		void cb(struct apk_package *pkg0, struct apk_dependency *dep0, struct apk_package *pkg, void *ctx),
		void *ctx);
void apk_pkg_foreach_reverse_dependency(
		struct apk_package *pkg, unsigned int match,
		void cb(struct apk_package *pkg0, struct apk_dependency *dep0, struct apk_package *pkg, void *ctx),
		void *ctx);
