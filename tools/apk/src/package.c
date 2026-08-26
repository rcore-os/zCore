/* package.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <errno.h>
#include <fcntl.h>
#include <ctype.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>

#include "apk_defines.h"
#include "apk_package.h"
#include "apk_database.h"
#include "apk_ctype.h"
#include "apk_print.h"
#include "apk_extract.h"
#include "apk_adb.h"

struct apk_package *apk_pkg_get_installed(struct apk_name *name)
{
	apk_array_foreach(p, name->providers)
		if (p->pkg->name == name && p->pkg->ipkg != NULL)
			return p->pkg;

	return NULL;
}

struct apk_installed_package *apk_pkg_install(struct apk_database *db,
					      struct apk_package *pkg)
{
	struct apk_installed_package *ipkg;

	if (pkg->ipkg != NULL)
		return pkg->ipkg;

	pkg->ipkg = ipkg = calloc(1, sizeof(struct apk_installed_package));
	ipkg->pkg = pkg;
	apk_string_array_init(&ipkg->triggers);
	apk_string_array_init(&ipkg->pending_triggers);
	apk_dependency_array_init(&ipkg->replaces);
	apk_db_dir_instance_array_init(&ipkg->diris);

	/* Overlay override information resides in a nameless package */
	if (pkg->name) {
		db->sorted_installed_packages = 0;
		db->installed.stats.packages++;
		db->installed.stats.bytes += pkg->installed_size;
		list_add_tail(&ipkg->installed_pkgs_list,
			      &db->installed.packages);
	}

	return ipkg;
}

void apk_pkg_uninstall(struct apk_database *db, struct apk_package *pkg)
{
	struct apk_installed_package *ipkg = pkg->ipkg;
	int i;

	if (ipkg == NULL)
		return;

	if (pkg->name) {
		list_del(&ipkg->installed_pkgs_list);
		if (db) {
			db->sorted_installed_packages = 0;
			db->installed.stats.packages--;
			db->installed.stats.bytes -= pkg->installed_size;
		}
	}

	if (apk_array_len(ipkg->triggers) != 0) {
		list_del(&ipkg->trigger_pkgs_list);
		list_init(&ipkg->trigger_pkgs_list);
		apk_array_foreach_item(trigger, ipkg->triggers)
			free(trigger);
	}
	apk_db_dir_instance_array_free(&ipkg->diris);
	apk_string_array_free(&ipkg->triggers);
	apk_string_array_free(&ipkg->pending_triggers);
	apk_dependency_array_free(&ipkg->replaces);

	for (i = 0; i < APK_SCRIPT_MAX; i++)
		if (ipkg->script[i].ptr != NULL)
			free(ipkg->script[i].ptr);
	free(ipkg);
	pkg->ipkg = NULL;
}

int apk_pkg_subst(void *ctx, apk_blob_t key, apk_blob_t *to)
{
	struct apk_package *pkg = ctx;
	if (apk_blob_compare(key, APK_BLOB_STRLIT("name")) == 0)
		apk_blob_push_blob(to, APK_BLOB_STR(pkg->name->name));
	else if (apk_blob_compare(key, APK_BLOB_STRLIT("version")) == 0)
		apk_blob_push_blob(to, *pkg->version);
	else if (apk_blob_compare(key, APK_BLOB_STRLIT("arch")) == 0)
		apk_blob_push_blob(to, *pkg->arch);
	else if (apk_blob_compare(key, APK_BLOB_STRLIT("hash")) == 0)
		apk_blob_push_hexdump(to, APK_BLOB_PTR_LEN((char *) pkg->digest, apk_digest_alg_len(pkg->digest_alg)));
	else
		return -APKE_PACKAGE_NAME_SPEC;
	return 0;
}

int apk_pkg_subst_validate(apk_blob_t fmt)
{
	char buf[1024];
	struct apk_name *name = alloca(sizeof(struct apk_name) + 5);
	struct apk_package *pkg = alloca(sizeof(struct apk_package));
	int r;

	// Validate that the syntax is valid
	*name = (struct apk_name) {};
	memcpy(name->name, "test", 5);
	*pkg = (struct apk_package) {
		.name = name,
		.version = &APK_BLOB_STRLIT("1"),
		.arch = &APK_BLOB_STRLIT("noarch"),
		.digest_alg = APK_DIGEST_NONE,
	};
	r = apk_blob_subst(buf, sizeof buf, fmt, apk_pkg_subst, pkg);
	if (r < 0) return r;

	// Validate that the final filename piece starts with ${name}[_-.]
	// so the reverse mapping code from filename to package works.
	apk_blob_rsplit(fmt, '/', NULL, &fmt);
	if (!apk_blob_starts_with(fmt, APK_BLOB_STRLIT("${name}"))) return -APKE_PACKAGE_NAME_SPEC;
	if (fmt.len < 8 || strchr("_-.", fmt.ptr[7]) == NULL) return -APKE_PACKAGE_NAME_SPEC;
	return 0;
}

int apk_dep_parse(apk_blob_t spec, apk_blob_t *name, int *rop, apk_blob_t *version)
{
	apk_blob_t bop;
	int op = 0;

	/* [!]name[[op]ver] */
	if (APK_BLOB_IS_NULL(spec)) goto fail;
	if (apk_blob_pull_blob_match(&spec, APK_BLOB_STRLIT("!")))
		op |= APK_VERSION_CONFLICT;
	if (apk_blob_cspn(spec, APK_CTYPE_DEPENDENCY_COMPARER, name, &bop)) {
		if (!apk_blob_spn(bop, APK_CTYPE_DEPENDENCY_COMPARER, &bop, version)) goto fail;
		op |= apk_version_result_mask_blob(bop);
		if ((op & ~APK_VERSION_CONFLICT) == 0) goto fail;
	} else {
		*name = spec;
		op |= APK_DEPMASK_ANY;
		*version = APK_BLOB_NULL;
	}
	*rop = op;
	return 0;
fail:
	*name = APK_BLOB_NULL;
	*version = APK_BLOB_NULL;
	*rop = APK_DEPMASK_ANY;
	return -APKE_DEPENDENCY_FORMAT;
}

void apk_deps_add(struct apk_dependency_array **deps, struct apk_dependency *dep)
{
	apk_array_foreach(d0, *deps) {
		if (d0->name != dep->name) continue;
		*d0 = *dep;
		return;
	}
	apk_dependency_array_add(deps, *dep);
}

void apk_deps_del(struct apk_dependency_array **pdeps, struct apk_name *name)
{
	struct apk_dependency_array *deps = *pdeps;

	apk_array_foreach(d0, deps) {
		if (d0->name != name) continue;
		size_t nlen = apk_array_len(deps) - 1;
		*d0 = deps->item[nlen];
		apk_array_truncate(*pdeps, nlen);
		return;
	}
}

void apk_blob_pull_dep(apk_blob_t *b, struct apk_database *db, struct apk_dependency *dep, bool allow_tag)
{
	struct apk_name *name;
	apk_blob_t bdep, bname, bver, btag;
	int op, tag = 0, broken = 0;

	/* grap one token, and skip all separators */
	if (APK_BLOB_IS_NULL(*b)) goto fail;
	apk_blob_cspn(*b, APK_CTYPE_DEPENDENCY_SEPARATOR, &bdep, b);
	apk_blob_spn(*b, APK_CTYPE_DEPENDENCY_SEPARATOR, NULL, b);

	if (apk_dep_parse(bdep, &bname, &op, &bver) != 0) goto fail;
	if ((op & APK_DEPMASK_CHECKSUM) != APK_DEPMASK_CHECKSUM &&
	    !apk_version_validate(bver)) broken = 1;
	if (apk_blob_split(bname, APK_BLOB_STRLIT("@"), &bname, &btag)) {
		if (!allow_tag) goto fail;
		tag = apk_db_get_tag_id(db, btag);
	}

	/* convert to apk_dependency */
	name = apk_db_get_name(db, bname);
	if (name == NULL) goto fail;

	*dep = (struct apk_dependency){
		.name = name,
		.version = apk_atomize_dup(&db->atoms, bver),
		.repository_tag = tag,
		.op = op,
		.broken = broken,
	};
	return;
fail:
	*dep = (struct apk_dependency){ .name = NULL };
	*b = APK_BLOB_NULL;
}

int apk_blob_pull_deps(apk_blob_t *b, struct apk_database *db, struct apk_dependency_array **deps, bool allow_tag)
{
	int rc = 0;

	while (b->len > 0) {
		struct apk_dependency dep;

		apk_blob_pull_dep(b, db, &dep, allow_tag);
		if (APK_BLOB_IS_NULL(*b) || dep.name == NULL) {
			rc = -APKE_DEPENDENCY_FORMAT;
			continue;
		}
		if (dep.broken) rc = -APKE_PKGVERSION_FORMAT;
		apk_dependency_array_add(deps, dep);
	}
	return rc;
}

void apk_dep_from_pkg(struct apk_dependency *dep, struct apk_database *db,
		      struct apk_package *pkg)
{
	char buf[64];
	apk_blob_t b = APK_BLOB_BUF(buf);

	apk_blob_push_hash(&b, apk_pkg_hash_blob(pkg));
	b = apk_blob_pushed(APK_BLOB_BUF(buf), b);

	*dep = (struct apk_dependency) {
		.name = pkg->name,
		.version = apk_atomize_dup(&db->atoms, b),
		.op = APK_DEPMASK_CHECKSUM,
	};
}

static int apk_dep_match_checksum(const struct apk_dependency *dep, const struct apk_package *pkg)
{
	struct apk_digest d;
	apk_blob_t b = *dep->version;

	apk_blob_pull_digest(&b, &d);
	return apk_blob_compare(APK_DIGEST_BLOB(d), apk_pkg_hash_blob(pkg)) == 0;
}

int apk_dep_is_provided(const struct apk_package *deppkg, const struct apk_dependency *dep, const struct apk_provider *p)
{
	if (p == NULL || p->pkg == NULL) return apk_dep_conflict(dep);
	if (apk_dep_conflict(dep) && deppkg == p->pkg) return 1;
	if (dep->op == APK_DEPMASK_CHECKSUM) return apk_dep_match_checksum(dep, p->pkg);
	return apk_version_match(*p->version, dep->op, *dep->version);
}

int apk_dep_is_materialized(const struct apk_dependency *dep, const struct apk_package *pkg)
{
	if (pkg == NULL || dep->name != pkg->name) return apk_dep_conflict(dep);
	if (dep->op == APK_DEPMASK_CHECKSUM) return apk_dep_match_checksum(dep, pkg);
	return apk_version_match(*pkg->version, dep->op, *dep->version);
}

int apk_dep_analyze(const struct apk_package *deppkg, struct apk_dependency *dep, struct apk_package *pkg)
{
	struct apk_provider provider;

	if (!pkg) return APK_DEP_IRRELEVANT;
	if (dep->name == pkg->name)
		return apk_dep_is_materialized(dep, pkg) ? APK_DEP_SATISFIES : APK_DEP_CONFLICTS;

	apk_array_foreach(p, pkg->provides) {
		if (p->name != dep->name) continue;
		provider = APK_PROVIDER_FROM_PROVIDES(pkg, p);
		return apk_dep_is_provided(deppkg, dep, &provider) ? APK_DEP_SATISFIES : APK_DEP_CONFLICTS;
	}
	return APK_DEP_IRRELEVANT;
}

void apk_blob_push_dep(apk_blob_t *to, struct apk_database *db, struct apk_dependency *dep)
{
	if (apk_dep_conflict(dep))
		apk_blob_push_blob(to, APK_BLOB_PTR_LEN("!", 1));

	apk_blob_push_blob(to, APK_BLOB_STR(dep->name->name));
	if (dep->repository_tag && db != NULL)
		apk_blob_push_blob(to, db->repo_tags[dep->repository_tag].tag);
	if (!APK_BLOB_IS_NULL(*dep->version)) {
		apk_blob_push_blob(to, APK_BLOB_STR(apk_version_op_string(dep->op)));
		apk_blob_push_blob(to, *dep->version);
	}
}

void apk_blob_push_deps(apk_blob_t *to, struct apk_database *db, struct apk_dependency_array *deps)
{
	if (!deps) return;
	apk_array_foreach(dep, deps) {
		if (dep != &deps->item[0]) apk_blob_push_blob(to, APK_BLOB_PTR_LEN(" ", 1));
		apk_blob_push_dep(to, db, dep);
	}
}

int apk_deps_write_layer(struct apk_database *db, struct apk_dependency_array *deps, struct apk_ostream *os, apk_blob_t separator, unsigned layer)
{
	apk_blob_t blob;
	char tmp[256];
	int n = 0;

	if (deps == NULL) return 0;
	apk_array_foreach(dep, deps) {
		if (layer != -1 && dep->layer != layer) continue;

		blob = APK_BLOB_BUF(tmp);
		if (n) apk_blob_push_blob(&blob, separator);
		apk_blob_push_dep(&blob, db, dep);

		blob = apk_blob_pushed(APK_BLOB_BUF(tmp), blob);
		if (APK_BLOB_IS_NULL(blob) ||
		    apk_ostream_write(os, blob.ptr, blob.len) < 0)
			return -1;

		n += blob.len;
	}

	return n;
}

int apk_deps_write(struct apk_database *db, struct apk_dependency_array *deps, struct apk_ostream *os, apk_blob_t separator)
{
	return apk_deps_write_layer(db, deps, os, separator, -1);
}

void apk_dep_from_adb(struct apk_dependency *dep, struct apk_database *db, struct adb_obj *d)
{
	int op = adb_ro_int(d, ADBI_DEP_MATCH);
	apk_blob_t ver = adb_ro_blob(d, ADBI_DEP_VERSION);

	if (APK_BLOB_IS_NULL(ver)) op |= APK_DEPMASK_ANY;
	else if (op == 0) op = APK_VERSION_EQUAL;

	*dep = (struct apk_dependency) {
		.name = apk_db_get_name(db, adb_ro_blob(d, ADBI_DEP_NAME)),
		.version = apk_atomize_dup(&db->atoms, ver),
		.op = op,
	};
}

void apk_deps_from_adb(struct apk_dependency_array **deps, struct apk_database *db, struct adb_obj *da)
{
	struct adb_obj obj;
	struct apk_dependency d;
	int i, num = adb_ra_num(da);

	apk_array_balloc(*deps, num, &db->ba_deps);
	for (i = ADBI_FIRST; i <= adb_ra_num(da); i++) {
		adb_ro_obj(da, i, &obj);
		apk_dep_from_adb(&d, db, &obj);
		apk_dependency_array_add(deps, d);
	}
}

const char *apk_script_types[] = {
	[APK_SCRIPT_PRE_INSTALL]	= "pre-install",
	[APK_SCRIPT_POST_INSTALL]	= "post-install",
	[APK_SCRIPT_PRE_DEINSTALL]	= "pre-deinstall",
	[APK_SCRIPT_POST_DEINSTALL]	= "post-deinstall",
	[APK_SCRIPT_PRE_UPGRADE]	= "pre-upgrade",
	[APK_SCRIPT_POST_UPGRADE]	= "post-upgrade",
	[APK_SCRIPT_TRIGGER]		= "trigger",
};

int apk_script_type(const char *name)
{
	int i;

	for (i = 0; i < ARRAY_SIZE(apk_script_types); i++)
		if (apk_script_types[i] &&
		    strcmp(apk_script_types[i], name) == 0)
			return i;

	return APK_SCRIPT_INVALID;
}

void apk_pkgtmpl_init(struct apk_package_tmpl *tmpl, struct apk_database *db)
{
	memset(tmpl, 0, sizeof *tmpl);
	tmpl->db = db;
	apk_dependency_array_init(&tmpl->pkg.depends);
	apk_dependency_array_init(&tmpl->pkg.install_if);
	apk_dependency_array_init(&tmpl->pkg.provides);
	apk_dependency_array_init(&tmpl->pkg.recommends);
	apk_blobptr_array_init(&tmpl->pkg.tags);
	apk_pkgtmpl_reset(tmpl);
}

void apk_pkgtmpl_free(struct apk_package_tmpl *tmpl)
{
	if (tmpl->pkg.ipkg) apk_pkg_uninstall(tmpl->db, &tmpl->pkg);
	apk_dependency_array_free(&tmpl->pkg.depends);
	apk_dependency_array_free(&tmpl->pkg.install_if);
	apk_dependency_array_free(&tmpl->pkg.provides);
	apk_dependency_array_free(&tmpl->pkg.recommends);
	apk_blobptr_array_free(&tmpl->pkg.tags);
}

void apk_pkgtmpl_reset(struct apk_package_tmpl *tmpl)
{
	*tmpl = (struct apk_package_tmpl) {
		.db = tmpl->db,
		.pkg = (struct apk_package) {
			.depends = apk_array_reset(tmpl->pkg.depends),
			.install_if = apk_array_reset(tmpl->pkg.install_if),
			.provides = apk_array_reset(tmpl->pkg.provides),
			.recommends = apk_array_reset(tmpl->pkg.recommends),
			.tags = apk_array_reset(tmpl->pkg.tags),
			.arch = &apk_atom_null,
			.license = &apk_atom_null,
			.origin = &apk_atom_null,
			.maintainer = &apk_atom_null,
			.url = &apk_atom_null,
			.description = &apk_atom_null,
			.commit = &apk_atom_null,
		},
	};
}

struct read_info_ctx {
	struct apk_extract_ctx ectx;
	struct apk_package_tmpl tmpl;
	int v3ok;
};

int apk_pkgtmpl_add_info(struct apk_package_tmpl *tmpl, char field, apk_blob_t value)
{
	struct apk_database *db = tmpl->db;
	struct apk_package *pkg = &tmpl->pkg;

	switch (field) {
	case 'P':
		pkg->name = apk_db_get_name(db, value);
		break;
	case 'V':
		pkg->version = apk_atomize_dup(&db->atoms, value);
		break;
	case 'T':
		pkg->description = apk_atomize_dup(&db->atoms, value);
		break;
	case 'U':
		pkg->url = apk_atomize_dup(&db->atoms, value);
		break;
	case 'L':
		pkg->license = apk_atomize_dup(&db->atoms, value);
		break;
	case 'A':
		pkg->arch = apk_atomize_dup(&db->atoms, value);
		break;
	case 'D':
		if (apk_blob_pull_deps(&value, db, &pkg->depends, false)) {
			db->compat_depversions = 1;
			db->compat_notinstallable = pkg->uninstallable = 1;
			return 2;
		}
		break;
	case 'C':
		apk_blob_pull_digest(&value, &tmpl->id);
		break;
	case 'S':
		pkg->size = apk_blob_pull_uint(&value, 10);
		break;
	case 'I':
		pkg->installed_size = apk_blob_pull_uint(&value, 10);
		break;
	case 'p':
		if (apk_blob_pull_deps(&value, db, &pkg->provides, false)) {
			db->compat_depversions = 1;
			return 2;
		}
		break;
	case 'i':
		if (apk_blob_pull_deps(&value, db, &pkg->install_if, false)) {
			// Disable partial install_if rules
			apk_array_truncate(pkg->install_if, 0);
			db->compat_depversions = 1;
			return 2;
		}
		break;
	case 'o':
		pkg->origin = apk_atomize_dup(&db->atoms, value);
		break;
	case 'm':
		pkg->maintainer = apk_atomize_dup(&db->atoms, value);
		break;
	case 't':
		pkg->build_time = apk_blob_pull_uint(&value, 10);
		break;
	case 'c':
		pkg->commit = apk_atomize_dup(&db->atoms, value);
		break;
	case 'k':
		pkg->provider_priority = apk_blob_pull_uint(&value, 10);
		break;
	case 'F': case 'M': case 'R': case 'Z': case 'r': case 'q':
	case 'a': case 's': case 'f': case 'g':
		/* installed db entries which are handled in database.c */
		return 1;
	default:
		/* lower case index entries are safe to be ignored */
		if (!islower(field)) db->compat_notinstallable = pkg->uninstallable = 1;
		db->compat_newfeatures = 1;
		return 2;
	}
	if (APK_BLOB_IS_NULL(value))
		return -APKE_V2PKG_FORMAT;
	return 0;
}

static apk_blob_t *commit_id(struct apk_atom_pool *atoms, apk_blob_t b)
{
	char buf[80];
	apk_blob_t to = APK_BLOB_BUF(buf);

	apk_blob_push_hexdump(&to, b);
	to = apk_blob_pushed(APK_BLOB_BUF(buf), to);
	if (APK_BLOB_IS_NULL(to)) return &apk_atom_null;
	return apk_atomize_dup(atoms, to);
}

static void apk_blobs_from_adb(struct apk_blobptr_array **arr, struct apk_database *db, struct adb_obj *da)
{
	apk_array_balloc(*arr, adb_ra_num(da), &db->ba_deps);
	for (int i = ADBI_FIRST; i <= adb_ra_num(da); i++)
		apk_blobptr_array_add(arr, apk_atomize_dup(&db->atoms, adb_ro_blob(da, i)));
}

void apk_pkgtmpl_from_adb(struct apk_package_tmpl *tmpl, struct adb_obj *pkginfo)
{
	struct apk_database *db = tmpl->db;
	struct adb_obj obj;
	struct apk_package *pkg = &tmpl->pkg;
	apk_blob_t uid;

	uid = adb_ro_blob(pkginfo, ADBI_PI_HASHES);
	if (uid.len >= APK_DIGEST_LENGTH_SHA1) apk_digest_from_blob(&tmpl->id, uid);

	pkg->name = apk_db_get_name(db, adb_ro_blob(pkginfo, ADBI_PI_NAME));
	pkg->version = apk_atomize_dup(&db->atoms, adb_ro_blob(pkginfo, ADBI_PI_VERSION));
	pkg->description = apk_atomize_dup(&db->atoms, apk_blob_truncate(adb_ro_blob(pkginfo, ADBI_PI_DESCRIPTION), 512));
	pkg->url = apk_atomize_dup(&db->atoms, adb_ro_blob(pkginfo, ADBI_PI_URL));
	pkg->license = apk_atomize_dup(&db->atoms, adb_ro_blob(pkginfo, ADBI_PI_LICENSE));
	pkg->arch = apk_atomize_dup(&db->atoms, adb_ro_blob(pkginfo, ADBI_PI_ARCH));
	pkg->installed_size = adb_ro_int(pkginfo, ADBI_PI_INSTALLED_SIZE);
	pkg->size = adb_ro_int(pkginfo, ADBI_PI_FILE_SIZE);
	pkg->provider_priority = adb_ro_int(pkginfo, ADBI_PI_PROVIDER_PRIORITY);
	pkg->origin = apk_atomize_dup(&db->atoms, adb_ro_blob(pkginfo, ADBI_PI_ORIGIN));
	pkg->maintainer = apk_atomize_dup(&db->atoms, adb_ro_blob(pkginfo, ADBI_PI_MAINTAINER));
	pkg->build_time = adb_ro_int(pkginfo, ADBI_PI_BUILD_TIME);
	pkg->commit = commit_id(&db->atoms, adb_ro_blob(pkginfo, ADBI_PI_REPO_COMMIT));
	pkg->layer = adb_ro_int(pkginfo, ADBI_PI_LAYER);

	apk_deps_from_adb(&pkg->depends, db, adb_ro_obj(pkginfo, ADBI_PI_DEPENDS, &obj));
	apk_deps_from_adb(&pkg->provides, db, adb_ro_obj(pkginfo, ADBI_PI_PROVIDES, &obj));
	apk_deps_from_adb(&pkg->install_if, db, adb_ro_obj(pkginfo, ADBI_PI_INSTALL_IF, &obj));
	apk_deps_from_adb(&pkg->recommends, db, adb_ro_obj(pkginfo, ADBI_PI_RECOMMENDS, &obj));
	apk_blobs_from_adb(&pkg->tags, db, adb_ro_obj(pkginfo, ADBI_PI_TAGS, &obj));
}

static int read_info_line(struct read_info_ctx *ri, apk_blob_t line)
{
	static struct {
		const char *str;
		char field;
	} fields[] = {
		{ "pkgname",	'P' },
		{ "pkgver", 	'V' },
		{ "pkgdesc",	'T' },
		{ "url",	'U' },
		{ "size",	'I' },
		{ "license",	'L' },
		{ "arch",	'A' },
		{ "depend",	'D' },
		{ "install_if",	'i' },
		{ "provides",	'p' },
		{ "origin",	'o' },
		{ "maintainer",	'm' },
		{ "builddate",	't' },
		{ "commit",	'c' },
		{ "provider_priority", 'k' },
	};
	apk_blob_t l, r;
	int i;

	if (line.ptr == NULL || line.len < 1 || line.ptr[0] == '#')
		return 0;

	if (!apk_blob_split(line, APK_BLOB_STR(" = "), &l, &r))
		return 0;

	apk_extract_v2_control(&ri->ectx, l, r);

	for (i = 0; i < ARRAY_SIZE(fields); i++)
		if (apk_blob_compare(APK_BLOB_STR(fields[i].str), l) == 0)
			return apk_pkgtmpl_add_info(&ri->tmpl, fields[i].field, r);

	return 0;
}

static int apk_pkg_v2meta(struct apk_extract_ctx *ectx, struct apk_istream *is)
{
	struct read_info_ctx *ri = container_of(ectx, struct read_info_ctx, ectx);
	apk_blob_t l, token = APK_BLOB_STR("\n");
	int r;

	while (apk_istream_get_delim(is, token, &l) == 0) {
		r = read_info_line(ri, l);
		if (r < 0) return r;
	}

	return 0;
}

static int apk_pkg_v3meta(struct apk_extract_ctx *ectx, struct adb_obj *pkg)
{
	struct read_info_ctx *ri = container_of(ectx, struct read_info_ctx, ectx);
	struct adb_obj pkginfo;

	if (!ri->v3ok) return -APKE_FORMAT_NOT_SUPPORTED;

	adb_ro_obj(pkg, ADBI_PKG_PKGINFO, &pkginfo);
	apk_pkgtmpl_from_adb(&ri->tmpl, &pkginfo);

	return -ECANCELED;
}

static const struct apk_extract_ops extract_pkgmeta_ops = {
	.v2meta = apk_pkg_v2meta,
	.v3meta = apk_pkg_v3meta,
};

int apk_pkg_read(struct apk_database *db, const char *file, struct apk_package **pkg, int v3ok)
{
	struct read_info_ctx ctx = { .v3ok = v3ok };
	struct apk_file_info fi;
	int r;

	r = apk_fileinfo_get(AT_FDCWD, file, 0, &fi, &db->atoms);
	if (r != 0) return r;

	apk_pkgtmpl_init(&ctx.tmpl, db);
	apk_extract_init(&ctx.ectx, db->ctx, &extract_pkgmeta_ops);
	apk_extract_generate_identity(&ctx.ectx, APK_DIGEST_SHA256, &ctx.tmpl.id);

	r = apk_extract(&ctx.ectx, apk_istream_from_file(AT_FDCWD, file));
	if (r < 0 && r != -ECANCELED) goto err;
	if (ctx.tmpl.id.alg == APK_DIGEST_NONE ||
	    ctx.tmpl.pkg.name == NULL ||
	    ctx.tmpl.pkg.uninstallable) {
		r = -APKE_V2PKG_FORMAT;
		goto err;
	}

	apk_string_array_add(&db->filename_array, (char*) file);
	ctx.tmpl.pkg.size = fi.size;
	ctx.tmpl.pkg.filename_ndx = apk_array_len(db->filename_array);

	if (pkg) *pkg = apk_db_pkg_add(db, &ctx.tmpl);
	else apk_db_pkg_add(db, &ctx.tmpl);
	r = 0;
err:
	apk_pkgtmpl_free(&ctx.tmpl);
	return r;
}

int apk_ipkg_assign_script(struct apk_installed_package *ipkg, unsigned int type, apk_blob_t b)
{
	if (APK_BLOB_IS_NULL(b)) return -1;
	if (type >= APK_SCRIPT_MAX) {
		free(b.ptr);
		return -1;
	}
	if (ipkg->script[type].ptr) free(ipkg->script[type].ptr);
	ipkg->script[type] = b;
	return 0;
}

int apk_ipkg_add_script(struct apk_installed_package *ipkg, struct apk_istream *is, unsigned int type, uint64_t size)
{
	apk_blob_t b;
	if (size > APK_MAX_SCRIPT_SIZE) return -APKE_FORMAT_INVALID;
	apk_blob_from_istream(is, size, &b);
	return apk_ipkg_assign_script(ipkg, type, b);
}

#ifdef __linux__
static inline int make_device_tree(struct apk_database *db)
{
	if (faccessat(db->root_fd, "dev", F_OK, 0) == 0) return 0;
	if (mkdirat(db->root_fd, "dev", 0755) < 0 ||
	    mknodat(db->root_fd, "dev/null", S_IFCHR | 0666, makedev(1, 3)) < 0 ||
	    mknodat(db->root_fd, "dev/zero", S_IFCHR | 0666, makedev(1, 5)) < 0 ||
	    mknodat(db->root_fd, "dev/random", S_IFCHR | 0666, makedev(1, 8)) < 0 ||
	    mknodat(db->root_fd, "dev/urandom", S_IFCHR | 0666, makedev(1, 9)) < 0 ||
	    mknodat(db->root_fd, "dev/console", S_IFCHR | 0600, makedev(5, 1)) < 0)
		return -1;
	return 0;
}
#else
static inline int make_device_tree(struct apk_database *db)
{
	(void) db;
	return 0;
}
#endif

int apk_ipkg_run_script(struct apk_installed_package *ipkg,
			struct apk_database *db,
			unsigned int type, char **argv)
{
	// When memfd_create is not available store the script in /lib/apk/exec
	// and hope it allows executing.
	static const char script_exec_dir[] = "lib/apk/exec";
	struct apk_out *out = &db->ctx->out;
	struct apk_package *pkg = ipkg->pkg;
	const char *reason = "failed to execute: ";
	char fn[PATH_MAX];
	int fd = -1, root_fd = db->root_fd, ret = 0, r;
	bool created = false;

	if (type >= APK_SCRIPT_MAX || ipkg->script[type].ptr == NULL) return 0;
	if ((db->ctx->flags & (APK_NO_SCRIPTS | APK_SIMULATE)) != 0) return 0;

	r = apk_fmt(fn, sizeof fn, "%s/" PKG_VER_FMT ".%s", script_exec_dir, PKG_VER_PRINTF(pkg), apk_script_types[type]);
	if (r < 0) goto err_r;

	argv[0] = fn;

	if (!db->memfd_failed) {
		/* Linux kernel >= 6.3 */
		fd = memfd_create(fn, MFD_EXEC|MFD_ALLOW_SEALING);
		if (fd < 0) db->memfd_failed = 1;
	}
	if (!db->script_dirs_checked) {
		if (fd < 0 && apk_make_dirs(root_fd, script_exec_dir, 0700, 0755) < 0) {
			reason = "failed to prepare dirs for hook scripts: ";
			goto err_errno;
		}
		if (!db->root_dev_ok && !db->need_unshare) {
			if (make_device_tree(db) < 0)
				apk_warn(out, PKG_VER_FMT ": failed to create initial device nodes: %s",
					 PKG_VER_PRINTF(pkg), apk_error_str(errno));
		}
		db->script_dirs_checked = 1;
	}
	if (fd < 0) {
		fd = openat(root_fd, fn, O_CREAT | O_RDWR | O_TRUNC, 0755);
		created = fd >= 0;
	}
	if (fd < 0) goto err_errno;

	if (write(fd, ipkg->script[type].ptr, ipkg->script[type].len) < 0)
		goto err_errno;

	if (created) {
		close(fd);
		fd = -1;
	} else {
#ifdef F_ADD_SEALS
		fcntl(fd, F_ADD_SEALS, F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW
			| F_SEAL_WRITE | F_SEAL_FUTURE_WRITE | F_SEAL_EXEC);
#endif
	}

	apk_msg(out, "%sExecuting " PKG_VER_FMT ".%s",
		db->indent_level ? "  " : "",
		PKG_VER_PRINTF(pkg), apk_script_types[type]);
	if (apk_db_run_script(db, apk_script_types[type], pkg->name->name, fd, argv, db->indent_level ? "  * " : "* " ) < 0)
		goto err;

	/* Script may have done something that changes id cache contents */
	apk_id_cache_reset(db->id_cache);
	goto cleanup;

err_errno:
	r = errno;
err_r:
	apk_err(out, PKG_VER_FMT ".%s: %s%s", PKG_VER_PRINTF(pkg), apk_script_types[type], reason, apk_error_str(r));
err:
	ipkg->broken_script = 1;
	ret = 1;
cleanup:
	if (fd >= 0) close(fd);
	if (created) unlinkat(root_fd, fn, 0);
	return ret;
}

static int write_depends(struct apk_ostream *os, const char *field,
			 struct apk_dependency_array *deps)
{
	int r;

	if (apk_array_len(deps) == 0) return 0;
	if (apk_ostream_write(os, field, 2) < 0) return -1;
	if ((r = apk_deps_write(NULL, deps, os, APK_BLOB_PTR_LEN(" ", 1))) < 0) return r;
	if (apk_ostream_write(os, "\n", 1) < 0) return -1;
	return 0;
}

int apk_pkg_write_index_header(struct apk_package *info, struct apk_ostream *os)
{
	char buf[2048];
	apk_blob_t bbuf = APK_BLOB_BUF(buf);

	apk_blob_push_blob(&bbuf, APK_BLOB_STR("C:"));
	apk_blob_push_hash(&bbuf, apk_pkg_hash_blob(info));
	apk_blob_push_blob(&bbuf, APK_BLOB_STR("\nP:"));
	apk_blob_push_blob(&bbuf, APK_BLOB_STR(info->name->name));
	apk_blob_push_blob(&bbuf, APK_BLOB_STR("\nV:"));
	apk_blob_push_blob(&bbuf, *info->version);
	if (info->arch->len) {
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("\nA:"));
		apk_blob_push_blob(&bbuf, *info->arch);
	}
	apk_blob_push_blob(&bbuf, APK_BLOB_STR("\nS:"));
	apk_blob_push_uint(&bbuf, info->size, 10);
	apk_blob_push_blob(&bbuf, APK_BLOB_STR("\nI:"));
	apk_blob_push_uint(&bbuf, info->installed_size, 10);
	apk_blob_push_blob(&bbuf, APK_BLOB_STR("\nT:"));
	apk_blob_push_blob(&bbuf, *info->description);
	apk_blob_push_blob(&bbuf, APK_BLOB_STR("\nU:"));
	apk_blob_push_blob(&bbuf, *info->url);
	apk_blob_push_blob(&bbuf, APK_BLOB_STR("\nL:"));
	apk_blob_push_blob(&bbuf, *info->license);
	if (info->origin->len) {
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("\no:"));
		apk_blob_push_blob(&bbuf, *info->origin);
	}
	if (info->maintainer->len) {
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("\nm:"));
		apk_blob_push_blob(&bbuf, *info->maintainer);
	}
	if (info->build_time) {
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("\nt:"));
		apk_blob_push_uint(&bbuf, info->build_time, 10);
	}
	if (info->commit->len) {
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("\nc:"));
		apk_blob_push_blob(&bbuf, *info->commit);
	}
	if (info->provider_priority) {
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("\nk:"));
		apk_blob_push_uint(&bbuf, info->provider_priority, 10);
	}
	apk_blob_push_blob(&bbuf, APK_BLOB_STR("\n"));

	if (APK_BLOB_IS_NULL(bbuf))
		return apk_ostream_cancel(os, -APKE_BUFFER_SIZE);

	bbuf = apk_blob_pushed(APK_BLOB_BUF(buf), bbuf);
	if (apk_ostream_write(os, bbuf.ptr, bbuf.len) < 0 ||
	    write_depends(os, "D:", info->depends) ||
	    write_depends(os, "p:", info->provides) ||
	    write_depends(os, "i:", info->install_if))
		return apk_ostream_cancel(os, -EIO);

	return 0;
}

int apk_pkg_write_index_entry(struct apk_package *pkg, struct apk_ostream *os)
{
	int r = apk_pkg_write_index_header(pkg, os);
	if (r < 0) return r;
	return apk_ostream_write(os, "\n", 1);
}

int apk_pkg_version_compare(const struct apk_package *a, const struct apk_package *b)
{
	if (a->version == b->version) return APK_VERSION_EQUAL;
	return apk_version_compare(*a->version, *b->version);
}

int apk_pkg_cmp_display(const struct apk_package *a, const struct apk_package *b)
{
	if (a->name != b->name)
		return apk_name_cmp_display(a->name, b->name);
	switch (apk_pkg_version_compare(a, b)) {
	case APK_VERSION_LESS:
		return -1;
	case APK_VERSION_GREATER:
		return 1;
	default:
		return 0;
	}
}

int apk_pkg_replaces_dir(const struct apk_package *a, const struct apk_package *b)
{
	/* Prefer overlay */
	if (!a) return APK_PKG_REPLACES_NO;
	if (!b) return APK_PKG_REPLACES_YES;

	/* Upgrading package? */
	if (a->name == b->name) return APK_PKG_REPLACES_YES;

	/* Replace files on removal */
	const struct apk_installed_package *ai = a->ipkg, *bi = b->ipkg;
	if (ai->to_be_removed) return APK_PKG_REPLACES_YES;
	if (bi->to_be_removed) return APK_PKG_REPLACES_NO;

	/* Highest replaces_priority wins */
	if (ai->replaces_priority > bi->replaces_priority) return APK_PKG_REPLACES_NO;
	if (ai->replaces_priority < bi->replaces_priority) return APK_PKG_REPLACES_YES;

	/* If both have the same origin... */
	if (a->origin->len && a->origin == b->origin) {
		/* .. and either has origin equal to package name, prefer it. */
		if (apk_blob_compare(*a->origin, APK_BLOB_STR(a->name->name)) == 0)
			return APK_PKG_REPLACES_NO;
		if (apk_blob_compare(*b->origin, APK_BLOB_STR(b->name->name)) == 0)
			return APK_PKG_REPLACES_YES;
	}

	/* Fall back to package name to have stable sort */
	if (strcmp(a->name->name, b->name->name) <= 0) return APK_PKG_REPLACES_NO;
	return APK_PKG_REPLACES_YES;
}

int apk_pkg_replaces_file(const struct apk_package *a, const struct apk_package *b)
{
	int a_prio = -1, b_prio = -1;

	/* Overlay file? Replace the ownership, but extraction will keep the overlay file. */
	if (!a) return APK_PKG_REPLACES_YES;
	if (!b) return APK_PKG_REPLACES_NO;

	/* Upgrading package? */
	if (a->name == b->name) return APK_PKG_REPLACES_YES;

	/* Replace files on removal */
	const struct apk_installed_package *ai = a->ipkg, *bi = b->ipkg;
	if (ai->to_be_removed) return APK_PKG_REPLACES_YES;
	if (bi->to_be_removed) return APK_PKG_REPLACES_NO;

	/* Does the original package replace the new one? */
	apk_array_foreach(dep, ai->replaces) {
		if (apk_dep_is_materialized(dep, b)) {
			a_prio = ai->replaces_priority;
			break;
		}
	}

	/* Does the new package replace the original one? */
	apk_array_foreach(dep, bi->replaces) {
		if (apk_dep_is_materialized(dep, a)) {
			b_prio = bi->replaces_priority;
			break;
		}
	}

	/* If the original package is more important, skip this file */
	if (a_prio > b_prio) return APK_PKG_REPLACES_NO;

	/* If the new package has valid 'replaces', we will overwrite
	 * the file without warnings. */
	if (b_prio >= 0) return APK_PKG_REPLACES_YES;

	/* Or same source package? */
	if (a->origin->len && a->origin == b->origin) return APK_PKG_REPLACES_YES;

	/* Both ship same file, but metadata is inconclusive. */
	return APK_PKG_REPLACES_CONFLICT;
}

unsigned int apk_foreach_genid(void)
{
	static unsigned int foreach_genid;
	foreach_genid += (~APK_FOREACH_GENID_MASK) + 1;
	return foreach_genid;
}

int apk_pkg_match_genid(struct apk_package *pkg, unsigned int match)
{
	unsigned int genid = match & APK_FOREACH_GENID_MASK;
	if (pkg && genid) {
		if (pkg->foreach_genid >= genid)
			return 1;
		pkg->foreach_genid = genid;
	}
	return 0;
}

void apk_pkg_foreach_matching_dependency(
		struct apk_package *pkg, struct apk_dependency_array *deps,
		unsigned int match, struct apk_package *mpkg,
		void cb(struct apk_package *pkg0, struct apk_dependency *dep0, struct apk_package *pkg, void *ctx),
		void *ctx)
{
	unsigned int one_dep_only = (match & APK_FOREACH_GENID_MASK) && !(match & APK_FOREACH_DEP);

	if (apk_pkg_match_genid(pkg, match)) return;
	apk_array_foreach(d, deps) {
		if (apk_dep_analyze(pkg, d, mpkg) & match) {
			cb(pkg, d, mpkg, ctx);
			if (one_dep_only) break;
		}
	}
}

static void foreach_reverse_dependency(
		struct apk_package *pkg,
		struct apk_name_array *rdepends,
		unsigned int match,
		void cb(struct apk_package *pkg0, struct apk_dependency *dep0, struct apk_package *pkg, void *ctx),
		void *ctx)
{
	unsigned int marked = match & APK_FOREACH_MARKED;
	unsigned int installed = match & APK_FOREACH_INSTALLED;
	unsigned int one_dep_only = (match & APK_FOREACH_GENID_MASK) && !(match & APK_FOREACH_DEP);
	unsigned int no_conflicts = (match & APK_FOREACH_NO_CONFLICTS);

	apk_array_foreach_item(name0, rdepends) {
		apk_array_foreach(p0, name0->providers) {
			struct apk_package *pkg0 = p0->pkg;
			if (installed && pkg0->ipkg == NULL) continue;
			if (marked && !pkg0->marked) continue;
			if (apk_pkg_match_genid(pkg0, match)) continue;
			apk_array_foreach(d0, pkg0->depends) {
				if (no_conflicts && apk_dep_conflict(d0)) continue;
				if (apk_dep_analyze(pkg0, d0, pkg) & match) {
					cb(pkg0, d0, pkg, ctx);
					if (one_dep_only) break;
				}
			}
		}
	}
}

void apk_pkg_foreach_reverse_dependency(
		struct apk_package *pkg, unsigned int match,
		void cb(struct apk_package *pkg0, struct apk_dependency *dep0, struct apk_package *pkg, void *ctx),
		void *ctx)
{
	foreach_reverse_dependency(pkg, pkg->name->rdepends, match, cb, ctx);
	apk_array_foreach(p, pkg->provides)
		foreach_reverse_dependency(pkg, p->name->rdepends, match, cb, ctx);
}
