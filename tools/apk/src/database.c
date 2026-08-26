/* database.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <errno.h>
#include <stdio.h>
#include <fcntl.h>
#include <libgen.h>
#include <unistd.h>
#include <sched.h>
#include <string.h>
#include <stdlib.h>
#include <signal.h>
#include <fnmatch.h>
#include <sys/file.h>
#include <sys/stat.h>

#ifdef __linux__
# include <stdarg.h>
# include <mntent.h>
# include <sys/vfs.h>
# include <sys/wait.h>
# include <sys/mount.h>
# include <sys/statvfs.h>
# include <linux/magic.h>
#endif

#include "apk_defines.h"
#include "apk_arch.h"
#include "apk_package.h"
#include "apk_database.h"
#include "apk_ctype.h"
#include "apk_extract.h"
#include "apk_process.h"
#include "apk_print.h"
#include "apk_tar.h"
#include "apk_adb.h"
#include "apk_fs.h"

static const char * const apk_static_cache_dir = "var/cache/apk";
static const char * const apk_world_file = "etc/apk/world";
static const char * const apk_arch_file = "etc/apk/arch";
static const char * const apk_lock_file = "lib/apk/db/lock";

static struct apk_db_acl *apk_default_acl_dir, *apk_default_acl_file;

static mode_t apk_db_dir_get_mode(struct apk_database *db, mode_t mode)
{
	// in usermode, return mode that makes the file readable for user
	if (db->usermode) return mode | S_IWUSR | S_IXUSR;
	return mode;
}

static apk_blob_t apk_pkg_ctx(struct apk_package *pkg)
{
	return APK_BLOB_PTR_LEN(pkg->name->name, strlen(pkg->name->name)+1);
}

static apk_blob_t pkg_name_get_key(apk_hash_item item)
{
	return APK_BLOB_STR(((struct apk_name *) item)->name);
}

static void pkg_name_free(struct apk_name *name)
{
	apk_provider_array_free(&name->providers);
	apk_name_array_free(&name->rdepends);
	apk_name_array_free(&name->rinstall_if);
}

static const struct apk_hash_ops pkg_name_hash_ops = {
	.node_offset = offsetof(struct apk_name, hash_node),
	.get_key = pkg_name_get_key,
	.hash_key = apk_blob_hash,
	.compare = apk_blob_compare,
	.delete_item = (apk_hash_delete_f) pkg_name_free,
};

static apk_blob_t pkg_info_get_key(apk_hash_item item)
{
	return apk_pkg_hash_blob(item);
}

static unsigned long csum_hash(apk_blob_t csum)
{
	/* Checksum's highest bits have the most "randomness", use that
	 * directly as hash */
	if (csum.len >= sizeof(uint32_t))
		return apk_unaligned_le32(csum.ptr);
	return 0;
}

static const struct apk_hash_ops pkg_info_hash_ops = {
	.node_offset = offsetof(struct apk_package, hash_node),
	.get_key = pkg_info_get_key,
	.hash_key = csum_hash,
	.compare = apk_blob_compare,
};

static apk_blob_t apk_db_dir_get_key(apk_hash_item item)
{
	struct apk_db_dir *dir = (struct apk_db_dir *) item;
	return APK_BLOB_PTR_LEN(dir->name, dir->namelen);
}

static const struct apk_hash_ops dir_hash_ops = {
	.node_offset = offsetof(struct apk_db_dir, hash_node),
	.get_key = apk_db_dir_get_key,
	.hash_key = apk_blob_hash,
	.compare = apk_blob_compare,
};

struct apk_db_file_hash_key {
	apk_blob_t dirname;
	apk_blob_t filename;
};

static unsigned long apk_db_file_hash_key(apk_blob_t _key)
{
	struct apk_db_file_hash_key *key = (struct apk_db_file_hash_key *) _key.ptr;

	return apk_blob_hash_seed(key->filename, apk_blob_hash(key->dirname));
}

static unsigned long apk_db_file_hash_item(apk_hash_item item)
{
	struct apk_db_file *dbf = (struct apk_db_file *) item;

	return apk_blob_hash_seed(APK_BLOB_PTR_LEN(dbf->name, dbf->namelen),
				  dbf->diri->dir->hash);
}

static int apk_db_file_compare_item(apk_hash_item item, apk_blob_t _key)
{
	struct apk_db_file *dbf = (struct apk_db_file *) item;
	struct apk_db_file_hash_key *key = (struct apk_db_file_hash_key *) _key.ptr;
	struct apk_db_dir *dir = dbf->diri->dir;
	int r;

	r = apk_blob_compare(key->filename,
			     APK_BLOB_PTR_LEN(dbf->name, dbf->namelen));
	if (r != 0)
		return r;

	r = apk_blob_compare(key->dirname,
			     APK_BLOB_PTR_LEN(dir->name, dir->namelen));
	return r;
}

static const struct apk_hash_ops file_hash_ops = {
	.node_offset = offsetof(struct apk_db_file, hash_node),
	.hash_key = apk_db_file_hash_key,
	.hash_item = apk_db_file_hash_item,
	.compare_item = apk_db_file_compare_item,
};

struct apk_name *apk_db_query_name(struct apk_database *db, apk_blob_t name)
{
	return (struct apk_name *) apk_hash_get(&db->available.names, name);
}

struct apk_name *apk_db_get_name(struct apk_database *db, apk_blob_t name)
{
	struct apk_name *pn;
	unsigned long hash = apk_hash_from_key(&db->available.names, name);

	pn = (struct apk_name *) apk_hash_get_hashed(&db->available.names, name, hash);
	if (pn != NULL)
		return pn;

	pn = apk_balloc_new_extra(&db->ba_names, struct apk_name, name.len+1);
	if (pn == NULL) return NULL;

	memset(pn, 0, sizeof *pn);
	memcpy(pn->name, name.ptr, name.len);
	pn->name[name.len] = 0;
	apk_provider_array_init(&pn->providers);
	apk_name_array_init(&pn->rdepends);
	apk_name_array_init(&pn->rinstall_if);
	apk_hash_insert_hashed(&db->available.names, pn, hash);
	db->sorted_names = 0;

	return pn;
}

static int cmp_provider(const void *a, const void *b)
{
	const struct apk_provider *pa = a, *pb = b;
	return apk_pkg_cmp_display(pa->pkg, pb->pkg);
}

struct apk_provider_array *apk_name_sorted_providers(struct apk_name *name)
{
	if (!name->providers_sorted) {
		apk_array_qsort(name->providers, cmp_provider);
		name->providers_sorted = 0;
	}
	return name->providers;
}

static struct apk_db_acl *__apk_db_acl_atomize(struct apk_database *db, mode_t mode, uid_t uid, gid_t gid, uint8_t hash_len, const uint8_t *hash)
{
	struct apk_db_acl *acl;
	apk_blob_t *b;

	acl = alloca(sizeof(*acl) + hash_len);
	acl->mode = mode & 07777;
	acl->uid = uid;
	acl->gid = gid;
	acl->xattr_hash_len = hash_len;

	if (hash_len) memcpy(acl->xattr_hash, hash, hash_len);

	b = apk_atomize_dup(&db->atoms, APK_BLOB_PTR_LEN((char*) acl, sizeof(*acl) + hash_len));
	return (struct apk_db_acl *) b->ptr;
}

static struct apk_db_acl *apk_db_acl_atomize(struct apk_database *db, mode_t mode, uid_t uid, gid_t gid)
{
	return __apk_db_acl_atomize(db, mode, uid, gid, 0, 0);
}

static struct apk_db_acl *apk_db_acl_atomize_digest(struct apk_database *db, mode_t mode, uid_t uid, gid_t gid, const struct apk_digest *dig)
{
	return __apk_db_acl_atomize(db, mode, uid, gid, dig->len, dig->data);
}

static int apk_db_dir_mkdir(struct apk_database *db, struct apk_fsdir *d, struct apk_db_acl *acl)
{
	if (db->ctx->flags & APK_SIMULATE) return 0;
	return apk_fsdir_create(d, apk_db_dir_get_mode(db, acl->mode), acl->uid, acl->gid);
}

void apk_db_dir_prepare(struct apk_database *db, struct apk_db_dir *dir, struct apk_db_acl *expected_acl, struct apk_db_acl *new_acl)
{
	struct apk_fsdir d;

	if (dir->namelen == 0) return;
	if (dir->created) return;
	dir->created = 1;

	if (dir->parent && !dir->parent->created)
		apk_db_dir_prepare(db, dir->parent, apk_default_acl_dir, apk_default_acl_dir);

	apk_fsdir_get(&d, APK_BLOB_PTR_LEN(dir->name, dir->namelen), db->extract_flags, db->ctx, APK_BLOB_NULL);
	if (!expected_acl) {
		/* Directory should not exist. Create it. */
		if (apk_db_dir_mkdir(db, &d, new_acl) == 0)
			dir->permissions_ok = 1;
		return;
	}

	switch (apk_fsdir_check(&d, apk_db_dir_get_mode(db, expected_acl->mode), expected_acl->uid, expected_acl->gid)) {
	case -ENOENT:
		if (apk_db_dir_mkdir(db, &d, new_acl) == 0)
			dir->permissions_ok = 1;
		break;
	case 0:
		dir->permissions_ok = 1;
		break;
	case APK_FS_DIR_MODIFIED:
	default:
		break;
	}
}

void apk_db_dir_unref(struct apk_database *db, struct apk_db_dir *dir, int rmdir_mode)
{
	if (--dir->refs > 0) return;
	db->installed.stats.dirs--;
	list_del(&dir->diris);
	if (dir->namelen != 0) {
		if (rmdir_mode == APK_DIR_REMOVE) {
			dir->modified = 1;
			if (!(db->ctx->flags & APK_SIMULATE)) {
				struct apk_fsdir d;
				apk_fsdir_get(&d, APK_BLOB_PTR_LEN(dir->name, dir->namelen),
					      db->extract_flags, db->ctx, APK_BLOB_NULL);
				apk_fsdir_delete(&d);
			}
		}
		apk_db_dir_unref(db, dir->parent, rmdir_mode);
		dir->parent = NULL;
	}
	dir->created = dir->permissions_ok = 0;
}

struct apk_db_dir *apk_db_dir_ref(struct apk_db_dir *dir)
{
	dir->refs++;
	return dir;
}

struct apk_db_dir *apk_db_dir_query(struct apk_database *db,
				    apk_blob_t name)
{
	return (struct apk_db_dir *) apk_hash_get(&db->installed.dirs, name);
}

struct apk_db_dir *apk_db_dir_get(struct apk_database *db, apk_blob_t name)
{
	struct apk_db_dir *dir;
	struct apk_protected_path_array *ppaths;
	apk_blob_t bparent;
	unsigned long hash = apk_hash_from_key(&db->installed.dirs, name);
	char *relative_name;

	name = apk_blob_trim_end(name, '/');
	dir = (struct apk_db_dir *) apk_hash_get_hashed(&db->installed.dirs, name, hash);
	if (dir != NULL && dir->refs) return apk_db_dir_ref(dir);
	if (dir == NULL) {
		dir = apk_balloc_new_extra(&db->ba_files, struct apk_db_dir, name.len+1);
		memset(dir, 0, sizeof *dir);
		dir->rooted_name[0] = '/';
		memcpy(dir->name, name.ptr, name.len);
		dir->name[name.len] = 0;
		dir->namelen = name.len;
		dir->hash = hash;
		apk_protected_path_array_init(&dir->protected_paths);
		apk_hash_insert_hashed(&db->installed.dirs, dir, hash);
	}

	db->installed.stats.dirs++;
	dir->refs = 1;
	list_init(&dir->diris);

	if (name.len == 0) {
		dir->parent = NULL;
		dir->has_protected_children = 1;
		ppaths = NULL;
	} else if (apk_blob_rsplit(name, '/', &bparent, NULL)) {
		dir->parent = apk_db_dir_get(db, bparent);
		dir->protect_mode = dir->parent->protect_mode;
		dir->has_protected_children = !apk_protect_mode_none(dir->protect_mode);
		ppaths = dir->parent->protected_paths;
	} else {
		dir->parent = apk_db_dir_get(db, APK_BLOB_NULL);
		ppaths = db->protected_paths;
	}

	if (ppaths == NULL)
		return dir;

	apk_array_reset(db->ic.ppaths);
	relative_name = strrchr(dir->rooted_name, '/') + 1;
	apk_array_foreach(ppath, ppaths) {
		char *slash = strchr(ppath->relative_pattern, '/');
		if (slash != NULL) {
			*slash = 0;
			if (fnmatch(ppath->relative_pattern, relative_name, FNM_PATHNAME) != 0) {
				*slash = '/';
				continue;
			}
			*slash = '/';

			apk_protected_path_array_add(&db->ic.ppaths, (struct apk_protected_path) {
				.relative_pattern = slash + 1,
				.protect_mode = ppath->protect_mode,
			});
		} else {
			if (fnmatch(ppath->relative_pattern, relative_name, FNM_PATHNAME) != 0)
				continue;

			dir->protect_mode = ppath->protect_mode;
		}
		dir->has_protected_children |= !apk_protect_mode_none(ppath->protect_mode);
	}
	dir->protected_paths = apk_array_bclone(db->ic.ppaths, &db->ba_files);

	return dir;
}

void apk_db_dir_update_permissions(struct apk_database *db, struct apk_db_dir_instance *diri)
{
	struct apk_db_dir *dir = diri->dir;
	struct apk_db_acl *acl = diri->acl;
	struct apk_fsdir d;
	char buf[APK_EXTRACTW_BUFSZ];
	int r;

	if (!dir->permissions_ok) return;
	if (db->ctx->flags & APK_SIMULATE) return;

	dir->modified = 1;
	apk_fsdir_get(&d, APK_BLOB_PTR_LEN(dir->name, dir->namelen), db->extract_flags, db->ctx, APK_BLOB_NULL);
	r = apk_fsdir_update_perms(&d, apk_db_dir_get_mode(db, acl->mode), acl->uid, acl->gid);
	if (r != 0) {
		apk_warn(&db->ctx->out, "failed to update directory %s: %s", dir->name, apk_extract_warning_str(r, buf, sizeof buf));
		db->num_dir_update_errors++;
	}
}

static void apk_db_dir_apply_diri_permissions(struct apk_database *db, struct apk_db_dir_instance *diri)
{
	struct apk_db_dir *dir = diri->dir;
	struct apk_db_acl *acl = diri->acl;

	if (dir->owner && apk_pkg_replaces_dir(dir->owner->pkg, diri->pkg) != APK_PKG_REPLACES_YES)
		return;

	// Check if the ACL changed and the directory needs update
	if (dir->owner && dir->owner->acl != acl) apk_db_dir_update_permissions(db, diri);
	dir->owner = diri;
}

static void apk_db_diri_remove(struct apk_database *db, struct apk_db_dir_instance *diri)
{
	list_del(&diri->dir_diri_list);
	if (diri->dir->owner == diri) {
		// Walk the directory instance to determine new owner
		struct apk_db_dir *dir = diri->dir;
		struct apk_db_dir_instance *di;
		dir->owner = NULL;
		list_for_each_entry(di, &dir->diris, dir_diri_list) {
			if (dir->owner == NULL ||
			    apk_pkg_replaces_dir(dir->owner->pkg, di->pkg) == APK_PKG_REPLACES_YES)
				dir->owner = di;
		}
		if (dir->owner) apk_db_dir_update_permissions(db, dir->owner);
	}
	apk_db_dir_unref(db, diri->dir, APK_DIR_REMOVE);
}

struct apk_db_file *apk_db_file_query(struct apk_database *db,
				      apk_blob_t dir,
				      apk_blob_t name)
{
	struct apk_db_file_hash_key key;

	key = (struct apk_db_file_hash_key) {
		.dirname = apk_blob_trim_end(dir, '/'),
		.filename = name,
	};
	return (struct apk_db_file *) apk_hash_get(&db->installed.files,
						   APK_BLOB_BUF(&key));
}

static int files_qsort_cmp(const void *p1, const void *p2)
{
	const struct apk_db_file *f1 = *(const struct apk_db_file * const*) p1;
	const struct apk_db_file *f2 = *(const struct apk_db_file * const*) p2;
	return apk_blob_sort(APK_BLOB_PTR_LEN((void*) f1->name, f1->namelen), APK_BLOB_PTR_LEN((void*) f2->name, f2->namelen));
}

static int files_bsearch_cmp(const void *key, const void *item)
{
	apk_blob_t name = *(const apk_blob_t *) key;
	const struct apk_db_file *fdb = *(const struct apk_db_file * const*) item;
	return apk_blob_sort(name, APK_BLOB_PTR_LEN((void*) fdb->name, fdb->namelen));
}


static struct apk_db_file *apk_db_file_new(struct apk_database *db,
					   struct apk_db_dir_instance *diri,
					   apk_blob_t name)
{
	struct apk_db_file *file;
	struct apk_ipkg_creator *ic = &db->ic;

	file = apk_balloc_new_extra(&db->ba_files, struct apk_db_file, name.len+1);
	if (file == NULL) return NULL;

	memset(file, 0, sizeof(*file));
	memcpy(file->name, name.ptr, name.len);
	file->name[name.len] = 0;
	file->namelen = name.len;
	file->diri = diri;
	file->acl = apk_default_acl_file;

	if (!ic->files_unsorted && apk_array_len(ic->files) > 0)
		ic->files_unsorted = files_qsort_cmp(&file, &ic->files->item[apk_array_len(ic->files)-1]) < 0;
	apk_db_file_array_add(&ic->files, file);

	return file;
}

static struct apk_db_file *apk_db_file_get(struct apk_database *db,
					   struct apk_db_dir_instance *diri,
					   apk_blob_t name)
{
	struct apk_db_file *file;
	struct apk_db_file_hash_key key;
	struct apk_db_dir *dir = diri->dir;
	unsigned long hash;

	key = (struct apk_db_file_hash_key) {
		.dirname = APK_BLOB_PTR_LEN(dir->name, dir->namelen),
		.filename = name,
	};

	hash = apk_blob_hash_seed(name, dir->hash);
	file = (struct apk_db_file *) apk_hash_get_hashed(
		&db->installed.files, APK_BLOB_BUF(&key), hash);
	if (file != NULL)
		return file;

	file = apk_db_file_new(db, diri, name);
	apk_hash_insert_hashed(&db->installed.files, file, hash);
	db->installed.stats.files++;

	return file;
}

static void add_name_to_array(struct apk_name *name, struct apk_name_array **a)
{
	apk_array_foreach_item(n, *a) if (n == name) return;
	apk_name_array_add(a, name);
}

static void apk_db_pkg_rdepends(struct apk_database *db, struct apk_package *pkg)
{
	apk_array_foreach(d, pkg->depends) {
		struct apk_name *rname = d->name;
		rname->is_dependency |= !apk_dep_conflict(d);
		add_name_to_array(pkg->name, &rname->rdepends);
		apk_array_foreach(p, pkg->provides) add_name_to_array(p->name, &rname->rdepends);
	}
	apk_array_foreach(d, pkg->install_if) {
		struct apk_name *rname = d->name;
		add_name_to_array(pkg->name, &rname->rinstall_if);
		apk_array_foreach(p, pkg->provides) add_name_to_array(p->name, &rname->rinstall_if);
	}
}

static int apk_db_parse_istream(struct apk_database *db, struct apk_istream *is, int (*cb)(struct apk_database *, apk_blob_t))
{
	apk_blob_t token = APK_BLOB_STRLIT("\n"), line;
	int r;

	if (IS_ERR(is)) return PTR_ERR(is);
	while (apk_istream_get_delim(is, token, &line) == 0) {
		r = cb(db, line);
		if (r < 0) {
			apk_istream_error(is, r);
			break;
		}
	}
	return apk_istream_close(is);
}

static int apk_db_add_arch(struct apk_database *db, apk_blob_t arch)
{
	apk_blob_t *atom;

	if (arch.len == 0) return 0;
	atom = apk_atomize_dup(&db->atoms, apk_blob_trim(arch));
	apk_array_foreach(item, db->arches)
		if (*item == atom) return 0;
	apk_blobptr_array_add(&db->arches, atom);
	return 0;
}

bool apk_db_arch_compatible(struct apk_database *db, apk_blob_t *arch)
{
	if (arch == &apk_atom_null) return true;
	apk_array_foreach(item, db->arches)
		if (*item == arch) return true;
	return db->noarch == arch;
}

const struct apk_package *apk_db_pkg_upgradable(const struct apk_database *db, const struct apk_package *pkg)
{
	struct apk_name *name = pkg->name;
	struct apk_package *ipkg = apk_pkg_get_installed(name);

	if (!ipkg) return NULL;

	unsigned short allowed_repos = db->repo_tags[ipkg->ipkg->repository_tag].allowed_repos;
	if (!(pkg->repos & allowed_repos)) return NULL;

	return apk_version_match(*ipkg->version, APK_VERSION_LESS, *pkg->version) ? ipkg : NULL;
}

struct apk_package *apk_db_pkg_add(struct apk_database *db, struct apk_package_tmpl *tmpl)
{
	struct apk_package *pkg = &tmpl->pkg, *idb;
	unsigned short old_repos = 0;

	if (!pkg->name || !pkg->version || tmpl->id.len < APK_DIGEST_LENGTH_SHA1) return NULL;
	if (!apk_db_arch_compatible(db, tmpl->pkg.arch)) tmpl->pkg.uninstallable = 1;

	idb = apk_hash_get(&db->available.packages, APK_BLOB_PTR_LEN((char*)tmpl->id.data, APK_DIGEST_LENGTH_SHA1));
	if (idb == NULL) {
		idb = apk_balloc_new_extra(&db->ba_pkgs, struct apk_package, tmpl->id.len);
		memcpy(idb, pkg, sizeof *pkg);
		memcpy(idb->digest, tmpl->id.data, tmpl->id.len);
		idb->digest_alg = tmpl->id.alg;
		if (idb->digest_alg == APK_DIGEST_SHA1 && idb->ipkg && idb->ipkg->sha256_160)
			idb->digest_alg = APK_DIGEST_SHA256_160;
		idb->ipkg = NULL;
		idb->depends = apk_array_bclone(pkg->depends, &db->ba_deps);
		idb->install_if = apk_array_bclone(pkg->install_if, &db->ba_deps);
		idb->provides = apk_array_bclone(pkg->provides, &db->ba_deps);
		idb->tags = apk_array_bclone(pkg->tags, &db->ba_deps);

		apk_hash_insert(&db->available.packages, idb);
		apk_provider_array_add(&idb->name->providers, APK_PROVIDER_FROM_PACKAGE(idb));
		apk_array_foreach(dep, idb->provides)
			apk_provider_array_add(&dep->name->providers, APK_PROVIDER_FROM_PROVIDES(idb, dep));
		if (db->open_complete)
			apk_db_pkg_rdepends(db, idb);
	} else {
		old_repos = idb->repos;
		idb->repos |= pkg->repos;
		if (!idb->filename_ndx) idb->filename_ndx = pkg->filename_ndx;
		if (!old_repos && idb->size != pkg->size) {
			idb->size = pkg->size;
			db->idb_dirty = 1;
		}
	}
	if (idb->repos && !old_repos) {
		pkg->name->has_repository_providers = 1;
		apk_array_foreach(dep, idb->provides)
			dep->name->has_repository_providers = 1;
	}

	if (idb->ipkg == NULL && pkg->ipkg != NULL) {
		apk_array_foreach_item(diri, pkg->ipkg->diris)
			diri->pkg = idb;
		idb->ipkg = pkg->ipkg;
		idb->ipkg->pkg = idb;
		pkg->ipkg = NULL;
	}
	apk_pkgtmpl_reset(tmpl);
	return idb;
}

static int apk_repo_fd(struct apk_database *db, struct apk_repository *repo, int *fd)
{
	if (!fd) return 0;
	if (repo == &db->cache_repository) {
		if (db->cache_fd < 0) return db->cache_fd;
		*fd = db->cache_fd;
	} else *fd = AT_FDCWD;
	return 0;
}

static int apk_repo_subst(void *ctx, apk_blob_t key, apk_blob_t *to)
{
	struct apk_repository *repo = ctx;
	if (apk_blob_compare(key, APK_BLOB_STRLIT("hash")) == 0)
		apk_blob_push_hexdump(to, APK_BLOB_PTR_LEN((char *) repo->hash.data, repo->hash.len));
	else
		return -APKE_FORMAT_INVALID;
	return 0;
}

int apk_repo_index_cache_url(struct apk_database *db, struct apk_repository *repo, int *fd, char *buf, size_t len)
{
	int r = apk_repo_fd(db, &db->cache_repository, fd);
	if (r < 0) return r;
	return apk_blob_subst(buf, len, APK_BLOB_STRLIT("APKINDEX.${hash:8}.tar.gz"), apk_repo_subst, repo);
}

int apk_repo_package_url(struct apk_database *db, struct apk_repository *repo, struct apk_package *pkg,
			 int *fd, char *buf, size_t len)
{
	int r = apk_repo_fd(db, repo, fd);
	if (r < 0) return r;

	if (repo == &db->filename_repository) {
		if (strlcpy(buf, db->filename_array->item[pkg->filename_ndx-1], len) >= len)
			return -ENAMETOOLONG;
		return 0;
	}

	r = 0;
	if (!repo->absolute_pkgname) {
		r = apk_fmt(buf, len, BLOB_FMT "/", BLOB_PRINTF(repo->url_base));
		if (r < 0) return r;
	}
	r = apk_blob_subst(&buf[r], len - r, repo->pkgname_spec, apk_pkg_subst, pkg);
	if (r < 0) return r;
	return 0;
}

int apk_cache_download(struct apk_database *db, struct apk_repository *repo, struct apk_package *pkg, struct apk_progress *prog)
{
	struct apk_out *out = &db->ctx->out;
	struct apk_progress_istream pis;
	struct apk_istream *is;
	struct apk_ostream *os;
	struct apk_extract_ctx ectx;
	char cache_filename[NAME_MAX], download_url[PATH_MAX];
	int r, download_fd, cache_fd, tee_flags = 0;
	time_t download_mtime = 0;

	if (pkg != NULL) {
		r = apk_repo_package_url(db, &db->cache_repository, pkg, &cache_fd, cache_filename, sizeof cache_filename);
		if (r < 0) return r;
		r = apk_repo_package_url(db, repo, pkg, &download_fd, download_url, sizeof download_url);
		if (r < 0) return r;
		tee_flags = APK_ISTREAM_TEE_COPY_META;
	} else {
		r = apk_repo_index_cache_url(db, repo, &cache_fd, cache_filename, sizeof cache_filename);
		if (r < 0) return r;
		download_mtime = repo->mtime;
		download_fd = AT_FDCWD;
		r = apk_fmt(download_url, sizeof download_url, BLOB_FMT, BLOB_PRINTF(repo->url_index));
		if (r < 0) return r;
		if (!prog) apk_out_progress_note(out, "fetch " BLOB_FMT, BLOB_PRINTF(repo->url_index_printable));
	}
	if (db->ctx->flags & APK_SIMULATE) return 0;

	os = apk_ostream_to_file_safe(cache_fd, cache_filename, 0644);
	if (IS_ERR(os)) return PTR_ERR(os);

	is = apk_istream_from_fd_url_if_modified(download_fd, download_url, apk_db_url_since(db, download_mtime));
	is = apk_progress_istream(&pis, is, prog);
	is = apk_istream_tee(is, os, tee_flags);
	apk_extract_init(&ectx, db->ctx, NULL);
	if (pkg) apk_extract_verify_identity(&ectx, pkg->digest_alg, apk_pkg_digest_blob(pkg));
	r = apk_extract(&ectx, is);
	if (r == -APKE_FILE_UNCHANGED) {
		if (!tee_flags) utimensat(cache_fd, cache_filename, NULL, 0);
		return r;
	}
	if (pkg) pkg->cached = 1;
	return r;
}

static void apk_db_ipkg_creator_reset(struct apk_ipkg_creator *ic)
{
	apk_array_reset(ic->diris);
	ic->num_unsorted_diris = 0;
	ic->diri = NULL;
}

static struct apk_installed_package *apk_db_ipkg_create(struct apk_database *db, struct apk_package *pkg)
{
	apk_db_ipkg_creator_reset(&db->ic);
	struct apk_installed_package *ipkg = apk_pkg_install(db, pkg);
	apk_db_dir_instance_array_copy(&db->ic.diris, ipkg->diris);
	return ipkg;
}

static void apk_db_ipkg_commit_files(struct apk_database *db)
{
	struct apk_ipkg_creator *ic = &db->ic;
	if (ic->diri) {
		if (ic->files_unsorted) apk_array_qsort(ic->files, files_qsort_cmp);
		ic->diri->files = apk_array_bclone(ic->files, &db->ba_files);
	}
	ic->files_unsorted = 0;
	apk_array_reset(db->ic.files);
}

static void apk_db_ipkg_commit(struct apk_database *db, struct apk_installed_package *ipkg)
{
	struct apk_ipkg_creator *ic = &db->ic;

	apk_db_ipkg_commit_files(db);
	ipkg->diris = apk_array_bclone(ic->diris, &db->ba_files);

	apk_array_foreach_item(diri, ipkg->diris)
		list_add_tail(&diri->dir_diri_list, &diri->dir->diris);

	apk_db_ipkg_creator_reset(ic);
}

static int diri_qsort_cmp(const void *p1, const void *p2)
{
	const struct apk_db_dir *d1 = (*(const struct apk_db_dir_instance * const*) p1)->dir;
	const struct apk_db_dir *d2 = (*(const struct apk_db_dir_instance * const*) p2)->dir;
	return apk_blob_sort(APK_BLOB_PTR_LEN((void*) d1->name, d1->namelen), APK_BLOB_PTR_LEN((void*) d2->name, d2->namelen));
}

static int diri_bsearch_cmp(const void *key, const void *elem)
{
	const apk_blob_t *dirname = key;
	const struct apk_db_dir *dir = (*(const struct apk_db_dir_instance * const*)elem)->dir;
	return apk_blob_sort(*dirname, APK_BLOB_PTR_LEN((void*) dir->name, dir->namelen));
}

static struct apk_db_dir_instance *apk_db_diri_bsearch(struct apk_database *db, apk_blob_t dirname)
{
	struct apk_ipkg_creator *ic = &db->ic;
	struct apk_db_dir_instance_array *diris = ic->diris;
	struct apk_db_dir_instance **entry;

	// Sort if sorting needed
	if (ic->num_unsorted_diris > 32) {
		apk_array_qsort(diris, diri_qsort_cmp);
		ic->num_unsorted_diris = 0;
	}

	// Search sorted portion
	int last_sorted = apk_array_len(diris) - ic->num_unsorted_diris;
	entry = bsearch(&dirname, diris->item, last_sorted, apk_array_item_size(diris), diri_bsearch_cmp);
	if (entry) return *entry;

	// Search non-sorted portion
	for (int i = last_sorted; i < apk_array_len(diris); i++)
		if (diri_bsearch_cmp(&dirname, &diris->item[i]) == 0)
			return diris->item[i];
	return NULL;
}

static struct apk_db_dir_instance *apk_db_diri_query(struct apk_database *db, apk_blob_t dirname)
{
	if (db->ic.diri && diri_bsearch_cmp(&dirname, &db->ic.diri) == 0) return db->ic.diri;
	return apk_db_diri_bsearch(db, dirname);
}

static struct apk_db_dir_instance *apk_db_diri_select(struct apk_database *db, struct apk_db_dir_instance *diri)
{
	struct apk_ipkg_creator *ic = &db->ic;

	if (diri == ic->diri) return diri;

	apk_db_ipkg_commit_files(db);

	ic->diri = diri;
	apk_db_file_array_copy(&ic->files, diri->files);

	return diri;
}

static struct apk_db_dir_instance *apk_db_diri_get(struct apk_database *db, apk_blob_t dirname, struct apk_package *pkg)
{
	struct apk_ipkg_creator *ic = &db->ic;
	struct apk_db_dir_instance *diri;
	int res = 1;

	if (ic->diri) {
		res = diri_bsearch_cmp(&dirname, &ic->diri);
		if (res == 0) return ic->diri;
	}

	diri = apk_db_diri_bsearch(db, dirname);
	if (!diri) {
		diri = apk_balloc_new(&db->ba_files, struct apk_db_dir_instance);
		if (!diri) return NULL;

		struct apk_db_dir *dir = apk_db_dir_get(db, dirname);
		list_init(&diri->dir_diri_list);
		diri->dir = dir;
		diri->pkg = pkg;
		diri->acl = apk_default_acl_dir;
		apk_db_file_array_init(&diri->files);

		if (ic->num_unsorted_diris)
			res = -1;
		else if (apk_array_len(ic->diris) && ic->diri != ic->diris->item[apk_array_len(ic->diris)-1])
			res = diri_bsearch_cmp(&dirname, &ic->diris->item[apk_array_len(ic->diris)-1]);
		if (res < 0) ic->num_unsorted_diris++;
		apk_db_dir_instance_array_add(&ic->diris, diri);
	}
	return apk_db_diri_select(db, diri);
}

static struct apk_db_file *apk_db_ipkg_find_file(struct apk_database *db, apk_blob_t file)
{
	struct apk_ipkg_creator *ic = &db->ic;

	apk_blob_t dir = APK_BLOB_NULL;
	apk_blob_rsplit(file, '/', &dir, &file);

	struct apk_db_dir_instance *diri = apk_db_diri_query(db, dir);
	if (!diri) return NULL;

	struct apk_db_file_array *files = diri->files;
	if (diri == ic->diri) {
		files = ic->files;
		if (ic->files_unsorted) {
			apk_array_qsort(files, files_qsort_cmp);
			ic->files_unsorted = 0;
		}
	}

	struct apk_db_file **entry = apk_array_bsearch(files, files_bsearch_cmp, &file);
	return entry ? *entry : NULL;
}

int apk_db_read_overlay(struct apk_database *db, struct apk_istream *is)
{
	struct apk_db_dir_instance *diri = NULL;
	struct apk_package *pkg = &db->overlay_tmpl.pkg;
	struct apk_installed_package *ipkg;
	apk_blob_t token = APK_BLOB_STR("\n"), line, bdir, bfile;

	if (IS_ERR(is)) return PTR_ERR(is);

	ipkg = apk_db_ipkg_create(db, pkg);
	if (ipkg == NULL) {
		apk_istream_error(is, -ENOMEM);
		goto err;
	}

	while (apk_istream_get_delim(is, token, &line) == 0) {
		if (!apk_blob_rsplit(line, '/', &bdir, &bfile)) {
			apk_istream_error(is, -APKE_V2PKG_FORMAT);
			break;
		}

		diri = apk_db_diri_get(db, bdir, NULL);
		if (bfile.len == 0) {
			diri->dir->created = 1;
		} else {
			apk_db_file_get(db, diri, bfile);
		}
	}
	apk_db_ipkg_commit(db, ipkg);
err:
	return apk_istream_close(is);
}

static int apk_db_fdb_read(struct apk_database *db, struct apk_istream *is, int repo, unsigned layer)
{
	struct apk_out *out = &db->ctx->out;
	struct apk_package_tmpl tmpl;
	struct apk_installed_package *ipkg = NULL;
	struct apk_db_dir_instance *diri = NULL;
	struct apk_db_file *file = NULL;
	struct apk_db_acl *acl;
	struct apk_digest file_digest, xattr_digest;
	apk_blob_t token = APK_BLOB_STR("\n"), l;
	mode_t mode;
	uid_t uid;
	gid_t gid;
	int field, r, lineno = 0;

	if (IS_ERR(is)) return PTR_ERR(is);

	apk_pkgtmpl_init(&tmpl, db);
	tmpl.pkg.layer = layer;

	while (apk_istream_get_delim(is, token, &l) == 0) {
		lineno++;

		if (l.len < 2) {
			if (!tmpl.pkg.name) continue;
			if (diri) apk_db_dir_apply_diri_permissions(db, diri);

			if (repo >= 0) {
				tmpl.pkg.repos |= BIT(repo);
			} else if (repo == APK_REPO_CACHE_INSTALLED) {
				tmpl.pkg.cached_non_repository = 1;
			} else if (repo == APK_REPO_DB_INSTALLED && ipkg == NULL) {
				/* Installed package without files */
				ipkg = apk_db_ipkg_create(db, &tmpl.pkg);
			}
			if (ipkg) apk_db_ipkg_commit(db, ipkg);
			if (apk_db_pkg_add(db, &tmpl) == NULL)
				goto err_fmt;

			tmpl.pkg.layer = layer;
			ipkg = NULL;
			diri = NULL;
			continue;
		}

		/* Get field */
		field = l.ptr[0];
		if (l.ptr[1] != ':') goto err_fmt;
		l.ptr += 2;
		l.len -= 2;

		/* Standard index line? */
		r = apk_pkgtmpl_add_info(&tmpl, field, l);
		if (r == 0) continue;
		if (r == 1 && repo == APK_REPO_DB_INSTALLED && ipkg == NULL) {
			/* Instert to installed database; this needs to
			 * happen after package name has been read, but
			 * before first FDB entry. */
			ipkg = apk_db_ipkg_create(db, &tmpl.pkg);
		}
		if (repo != APK_REPO_DB_INSTALLED || ipkg == NULL) continue;

		/* Check FDB special entries */
		switch (field) {
		case 'g':
			apk_blob_foreach_word(tag, l)
				apk_blobptr_array_add(&tmpl.pkg.tags, apk_atomize_dup(&db->atoms, tag));
			break;
		case 'F':
			if (tmpl.pkg.name == NULL) goto bad_entry;
			if (diri) apk_db_dir_apply_diri_permissions(db, diri);
			diri = apk_db_diri_get(db, l, &tmpl.pkg);
			break;
		case 'a':
			if (file == NULL) goto bad_entry;
		case 'M':
			if (diri == NULL) goto bad_entry;
			uid = apk_blob_pull_uint(&l, 10);
			apk_blob_pull_char(&l, ':');
			gid = apk_blob_pull_uint(&l, 10);
			apk_blob_pull_char(&l, ':');
			mode = apk_blob_pull_uint(&l, 8);
			if (apk_blob_pull_blob_match(&l, APK_BLOB_STR(":")))
				apk_blob_pull_digest(&l, &xattr_digest);
			else
				apk_digest_reset(&xattr_digest);

			acl = apk_db_acl_atomize_digest(db, mode, uid, gid, &xattr_digest);
			if (field == 'M')
				diri->acl = acl;
			else
				file->acl = acl;
			break;
		case 'R':
			if (diri == NULL) goto bad_entry;
			file = apk_db_file_get(db, diri, l);
			break;
		case 'Z':
			if (file == NULL) goto bad_entry;
			apk_blob_pull_digest(&l, &file_digest);
			if (file_digest.alg == APK_DIGEST_SHA1 && ipkg->sha256_160)
				apk_digest_set(&file_digest, APK_DIGEST_SHA256_160);
			apk_dbf_digest_set(file, file_digest.alg, file_digest.data);
			break;
		case 'r':
			apk_blob_pull_deps(&l, db, &ipkg->replaces, false);
			break;
		case 'q':
			ipkg->replaces_priority = apk_blob_pull_uint(&l, 10);
			break;
		case 's':
			ipkg->repository_tag = apk_db_get_tag_id(db, l);
			break;
		case 'f':
			for (r = 0; r < l.len; r++) {
				switch (l.ptr[r]) {
				case 'f': ipkg->broken_files = 1; break;
				case 's': ipkg->broken_script = 1; break;
				case 'x': ipkg->broken_xattr = 1; break;
				case 'S': ipkg->sha256_160 = 1; break;
				default:
					if (!(db->ctx->force & APK_FORCE_OLD_APK))
						goto old_apk_tools;
				}
			}
			break;
		default:
			if (r != 0 && !(db->ctx->force & APK_FORCE_OLD_APK))
				goto old_apk_tools;
			/* Installed. So mark the package as installable. */
			tmpl.pkg.filename_ndx = 0;
			continue;
		}
		if (APK_BLOB_IS_NULL(l)) goto bad_entry;
	}
	if (is->err < 0) goto err_fmt;
	goto done;

old_apk_tools:
	/* Installed db should not have unsupported fields */
	apk_err(out, "This apk-tools is too old to handle installed packages");
	goto err_fmt;
bad_entry:
	apk_err(out, "FDB format error (line %d, entry '%c')", lineno, field);
err_fmt:
	is->err = -APKE_V2DB_FORMAT;
done:
	apk_pkgtmpl_free(&tmpl);
	return apk_istream_close(is);
}

int apk_db_index_read(struct apk_database *db, struct apk_istream *is, int repo)
{
	return apk_db_fdb_read(db, is, repo, APK_DB_LAYER_ROOT);
}

static void apk_blob_push_db_acl(apk_blob_t *b, char field, struct apk_db_acl *acl)
{
	char hdr[2] = { field, ':' };

	apk_blob_push_blob(b, APK_BLOB_BUF(hdr));
	apk_blob_push_uint(b, acl->uid, 10);
	apk_blob_push_blob(b, APK_BLOB_STR(":"));
	apk_blob_push_uint(b, acl->gid, 10);
	apk_blob_push_blob(b, APK_BLOB_STR(":"));
	apk_blob_push_uint(b, acl->mode, 8);
	if (acl->xattr_hash_len != 0) {
		apk_blob_push_blob(b, APK_BLOB_STR(":"));
		apk_blob_push_hash(b, apk_acl_digest_blob(acl));
	}
	apk_blob_push_blob(b, APK_BLOB_STR("\n"));
}

static int write_blobs(struct apk_ostream *os, const char *field, struct apk_blobptr_array *blobs)
{
	apk_blob_t separator = APK_BLOB_STR(field);
	if (apk_array_len(blobs) == 0) return 0;
	apk_array_foreach_item(blob, blobs) {
		if (apk_ostream_write_blob(os, separator) < 0) goto err;
		if (apk_ostream_write_blob(os, *blob) < 0) goto err;
		separator = APK_BLOB_STRLIT(" ");
	}
	apk_ostream_write(os, "\n", 1);
err:
	return apk_ostream_error(os);
}

static int apk_db_fdb_write(struct apk_database *db, struct apk_installed_package *ipkg, struct apk_ostream *os)
{
	struct apk_package *pkg = ipkg->pkg;
	char buf[1024+PATH_MAX];
	apk_blob_t bbuf = APK_BLOB_BUF(buf);
	int r = 0;

	if (IS_ERR(os)) return PTR_ERR(os);

	r = apk_pkg_write_index_header(pkg, os);
	if (r < 0) goto err;

	r = write_blobs(os, "g:", pkg->tags);
	if (r < 0) goto err;

	if (apk_array_len(ipkg->replaces) != 0) {
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("r:"));
		apk_blob_push_deps(&bbuf, db, ipkg->replaces);
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("\n"));
	}
	if (ipkg->replaces_priority) {
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("q:"));
		apk_blob_push_uint(&bbuf, ipkg->replaces_priority, 10);
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("\n"));
	}
	if (ipkg->repository_tag) {
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("s:"));
		apk_blob_push_blob(&bbuf, db->repo_tags[ipkg->repository_tag].plain_name);
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("\n"));
	}
	if (ipkg->broken_files || ipkg->broken_script || ipkg->broken_xattr || ipkg->sha256_160) {
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("f:"));
		if (ipkg->broken_files)
			apk_blob_push_blob(&bbuf, APK_BLOB_STR("f"));
		if (ipkg->broken_script)
			apk_blob_push_blob(&bbuf, APK_BLOB_STR("s"));
		if (ipkg->broken_xattr)
			apk_blob_push_blob(&bbuf, APK_BLOB_STR("x"));
		if (ipkg->sha256_160)
			apk_blob_push_blob(&bbuf, APK_BLOB_STR("S"));
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("\n"));
	}
	apk_array_foreach_item(diri, ipkg->diris) {
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("F:"));
		apk_blob_push_blob(&bbuf, APK_BLOB_PTR_LEN(diri->dir->name, diri->dir->namelen));
		apk_blob_push_blob(&bbuf, APK_BLOB_STR("\n"));

		if (diri->acl != apk_default_acl_dir)
			apk_blob_push_db_acl(&bbuf, 'M', diri->acl);

		bbuf = apk_blob_pushed(APK_BLOB_BUF(buf), bbuf);
		if (APK_BLOB_IS_NULL(bbuf)) {
			r = -APKE_BUFFER_SIZE;
			goto err;
		}
		r = apk_ostream_write(os, bbuf.ptr, bbuf.len);
		if (r < 0) goto err;
		bbuf = APK_BLOB_BUF(buf);

		apk_array_foreach_item(file, diri->files) {
			if (file->audited) continue;

			apk_blob_push_blob(&bbuf, APK_BLOB_STR("R:"));
			apk_blob_push_blob(&bbuf, APK_BLOB_PTR_LEN(file->name, file->namelen));
			apk_blob_push_blob(&bbuf, APK_BLOB_STR("\n"));

			if (file->acl != apk_default_acl_file)
				apk_blob_push_db_acl(&bbuf, 'a', file->acl);

			if (file->digest_alg != APK_DIGEST_NONE) {
				apk_blob_push_blob(&bbuf, APK_BLOB_STR("Z:"));
				apk_blob_push_hash(&bbuf, apk_dbf_digest_blob(file));
				apk_blob_push_blob(&bbuf, APK_BLOB_STR("\n"));
			}

			bbuf = apk_blob_pushed(APK_BLOB_BUF(buf), bbuf);
			if (APK_BLOB_IS_NULL(bbuf)) {
				r = -APKE_BUFFER_SIZE;
				goto err;
			}
			r = apk_ostream_write(os, bbuf.ptr, bbuf.len);
			if (r < 0) goto err;
			bbuf = APK_BLOB_BUF(buf);
		}
	}
	r = apk_ostream_write(os, "\n", 1);
err:
	if (r < 0) apk_ostream_cancel(os, r);
	return r;
}

static int apk_db_scriptdb_write(struct apk_database *db, struct apk_installed_package *ipkg, struct apk_ostream *os)
{
	struct apk_package *pkg = ipkg->pkg;
	struct apk_file_info fi;
	char filename[256];
	apk_blob_t bfn;
	int r, i;

	if (IS_ERR(os)) return PTR_ERR(os);

	for (i = 0; i < APK_SCRIPT_MAX; i++) {
		if (!ipkg->script[i].ptr) continue;

		fi = (struct apk_file_info) {
			.name = filename,
			.size = ipkg->script[i].len,
			.mode = 0755 | S_IFREG,
			.mtime = pkg->build_time,
		};
		/* The scripts db expects file names in format:
		 * pkg-version.<hexdump of package checksum>.action */
		bfn = APK_BLOB_BUF(filename);
		apk_blob_push_blob(&bfn, APK_BLOB_STR(pkg->name->name));
		apk_blob_push_blob(&bfn, APK_BLOB_STR("-"));
		apk_blob_push_blob(&bfn, *pkg->version);
		apk_blob_push_blob(&bfn, APK_BLOB_STR("."));
		apk_blob_push_hash_hex(&bfn, apk_pkg_hash_blob(pkg));
		apk_blob_push_blob(&bfn, APK_BLOB_STR("."));
		apk_blob_push_blob(&bfn, APK_BLOB_STR(apk_script_types[i]));
		apk_blob_push_blob(&bfn, APK_BLOB_PTR_LEN("", 1));

		r = apk_tar_write_entry(os, &fi, ipkg->script[i].ptr);
		if (r < 0) {
			apk_ostream_cancel(os, -APKE_V2DB_FORMAT);
			break;
		}
	}

	return r;
}

static int apk_read_script_archive_entry(void *ctx,
					 const struct apk_file_info *ae,
					 struct apk_istream *is)
{
	struct apk_database *db = (struct apk_database *) ctx;
	struct apk_package *pkg;
	char *fncsum, *fnaction;
	struct apk_digest digest;
	apk_blob_t blob;
	int type;

	if (!S_ISREG(ae->mode))
		return 0;

	/* The scripts db expects file names in format:
	 * pkgname-version.<hexdump of package checksum>.action */
	fnaction = memrchr(ae->name, '.', strlen(ae->name));
	if (fnaction == NULL || fnaction == ae->name)
		return 0;
	fncsum = memrchr(ae->name, '.', fnaction - ae->name - 1);
	if (fncsum == NULL)
		return 0;
	fnaction++;
	fncsum++;

	/* Parse it */
	type = apk_script_type(fnaction);
	if (type == APK_SCRIPT_INVALID)
		return 0;
	blob = APK_BLOB_PTR_PTR(fncsum, fnaction - 2);
	apk_blob_pull_digest(&blob, &digest);

	/* Attach script */
	pkg = apk_db_get_pkg(db, &digest);
	if (pkg != NULL && pkg->ipkg != NULL)
		apk_ipkg_add_script(pkg->ipkg, is, type, ae->size);

	return 0;
}

static int apk_db_triggers_write(struct apk_database *db, struct apk_installed_package *ipkg, struct apk_ostream *os)
{
	char buf[APK_BLOB_DIGEST_BUF];
	apk_blob_t bfn;

	if (IS_ERR(os)) return PTR_ERR(os);
	if (apk_array_len(ipkg->triggers) == 0) return 0;

	bfn = APK_BLOB_BUF(buf);
	apk_blob_push_hash(&bfn, apk_pkg_hash_blob(ipkg->pkg));
	bfn = apk_blob_pushed(APK_BLOB_BUF(buf), bfn);
	apk_ostream_write(os, bfn.ptr, bfn.len);

	apk_array_foreach_item(trigger, ipkg->triggers) {
		apk_ostream_write(os, " ", 1);
		apk_ostream_write_string(os, trigger);
	}
	apk_ostream_write(os, "\n", 1);
	return 0;
}

static void apk_db_pkg_add_triggers(struct apk_database *db, struct apk_installed_package *ipkg, apk_blob_t triggers)
{
	apk_blob_foreach_word(word, triggers)
		apk_string_array_add(&ipkg->triggers, apk_blob_cstr(word));

	if (apk_array_len(ipkg->triggers) != 0 &&
	    !list_hashed(&ipkg->trigger_pkgs_list))
		list_add_tail(&ipkg->trigger_pkgs_list,
			      &db->installed.triggers);
}

static int apk_db_add_trigger(struct apk_database *db, apk_blob_t l)
{
	struct apk_digest digest;
	struct apk_package *pkg;

	apk_blob_pull_digest(&l, &digest);
	apk_blob_pull_char(&l, ' ');
	pkg = apk_db_get_pkg(db, &digest);
	if (pkg && pkg->ipkg) apk_db_pkg_add_triggers(db, pkg->ipkg, l);
	return 0;
}

static int apk_db_read_layer(struct apk_database *db, unsigned layer)
{
	apk_blob_t blob, world;
	int r, fd, ret = 0, flags = db->ctx->open_flags;

	/* Read:
	 * 1. world
	 * 2. installed packages db
	 * 3. triggers db
	 * 4. scripts db
	 */

	fd = openat(db->root_fd, apk_db_layer_name(layer), O_RDONLY | O_CLOEXEC | O_DIRECTORY);
	if (fd < 0) return -errno;

	if (!(flags & APK_OPENF_NO_WORLD)) {
		if (layer == APK_DB_LAYER_ROOT)
			ret = apk_blob_from_file(db->root_fd, apk_world_file, &world);
		else
			ret = apk_blob_from_file(fd, "world", &world);

		if (!ret) {
			blob = apk_blob_trim(world);
			ret = apk_blob_pull_deps(&blob, db, &db->world, true);
			free(world.ptr);
		} else if (layer == APK_DB_LAYER_ROOT) {
			ret = -ENOENT;
		}
	}

	if (!(flags & APK_OPENF_NO_INSTALLED)) {
		r = apk_db_fdb_read(db, apk_istream_from_file(fd, "installed"), APK_REPO_DB_INSTALLED, layer);
		if (!ret && r != -ENOENT) ret = r;
		r = apk_db_parse_istream(db, apk_istream_from_file(fd, "triggers"), apk_db_add_trigger);
		if (!ret && r != -ENOENT) ret = r;
	}

	if (!(flags & APK_OPENF_NO_SCRIPTS)) {
		struct apk_istream *is = apk_istream_from_file(fd, "scripts.tar");
		if (!IS_ERR(is) || PTR_ERR(is) != -ENOENT) db->scripts_tar = 1;
		else is = apk_istream_gunzip(apk_istream_from_file(fd, "scripts.tar.gz"));

		r = apk_tar_parse(is, apk_read_script_archive_entry, db, db->id_cache);
		if (!ret && r != -ENOENT) ret = r;
	}

	close(fd);
	return ret;
}

static int apk_db_index_write_nr_cache(struct apk_database *db)
{
	struct apk_ostream *os = NULL;

	if (apk_db_permanent(db) || !apk_db_cache_active(db)) return 0;

	/* Write list of installed non-repository packages to
	 * cached index file */
	struct apk_package_array *pkgs = apk_db_sorted_installed_packages(db);
	apk_array_foreach_item(pkg, pkgs) {
		if (apk_db_pkg_available(db, pkg)) continue;
		if (pkg->cached || pkg->filename_ndx || !pkg->installed_size) {
			if (!os) {
				os = apk_ostream_to_file(db->cache_fd, "installed", 0644);
				if (IS_ERR(os)) return PTR_ERR(os);
			}
			if (apk_pkg_write_index_entry(pkg, os) < 0) break;
		}
	}
	if (os) return apk_ostream_close(os);
	/* Nothing written, remove existing file if any */
	unlinkat(db->cache_fd, "installed", 0);
	return 0;
}

static int apk_db_add_protected_path(struct apk_database *db, apk_blob_t blob)
{
	int protect_mode = APK_PROTECT_NONE;

	/* skip empty lines and comments */
	if (blob.len == 0)
		return 0;

	switch (blob.ptr[0]) {
	case '#':
		return 0;
	case '-':
		protect_mode = APK_PROTECT_IGNORE;
		break;
	case '+':
		protect_mode = APK_PROTECT_CHANGED;
		break;
	case '@':
		protect_mode = APK_PROTECT_SYMLINKS_ONLY;
		break;
	case '!':
		protect_mode = APK_PROTECT_ALL;
		break;
	default:
		protect_mode = APK_PROTECT_CHANGED;
		goto no_mode_char;
	}
	blob.ptr++;
	blob.len--;

no_mode_char:
	/* skip leading and trailing path separators */
	blob = apk_blob_trim_start(blob, '/');
	blob = apk_blob_trim_end(blob, '/');
	apk_protected_path_array_add(&db->protected_paths, (struct apk_protected_path) {
		.relative_pattern = apk_balloc_cstr(&db->ctx->ba, blob),
		.protect_mode = protect_mode,
	});
	return 0;
}

static bool file_not_dot_list(const char *file)
{
	if (apk_filename_is_hidden(file)) return true;
	const char *ext = strrchr(file, '.');
	return (ext && strcmp(ext, ".list") == 0) ? false : true;
}

static int add_protected_paths_from_file(void *ctx, int dirfd, const char *path, const char *file)
{
	apk_db_parse_istream((struct apk_database *) ctx, apk_istream_from_file(dirfd, file), apk_db_add_protected_path);
	return 0;
}

static void handle_alarm(int sig)
{
}

static void mark_in_cache(struct apk_database *db, int static_cache, int dirfd, const char *name, struct apk_package *pkg)
{
	if (!pkg) return;
	pkg->cached = 1;
}

struct apkindex_ctx {
	struct apk_database *db;
	struct apk_extract_ctx ectx;
	int repo, found;
};

static int load_v2index(struct apk_extract_ctx *ectx, apk_blob_t *desc, struct apk_istream *is)
{
	struct apkindex_ctx *ctx = container_of(ectx, struct apkindex_ctx, ectx);
	if (ctx->repo >= 0) {
		struct apk_repository *repo = &ctx->db->repos[ctx->repo];
		if (!repo->v2_allowed) return -APKE_FORMAT_INVALID;
		repo->description = *apk_atomize_dup(&ctx->db->atoms, *desc);
	}
	return apk_db_index_read(ctx->db, is, ctx->repo);
}

static int load_v3index(struct apk_extract_ctx *ectx, struct adb_obj *ndx)
{
	struct apkindex_ctx *ctx = container_of(ectx, struct apkindex_ctx, ectx);
	struct apk_database *db = ctx->db;
	struct apk_out *out = &db->ctx->out;
	struct apk_repository *repo = &db->repos[ctx->repo];
	struct apk_package_tmpl tmpl;
	struct adb_obj pkgs, pkginfo;
	apk_blob_t pkgname_spec;
	int i, r = 0, num_broken = 0;

	apk_pkgtmpl_init(&tmpl, db);

	repo->description = *apk_atomize_dup(&db->atoms, adb_ro_blob(ndx, ADBI_NDX_DESCRIPTION));
	pkgname_spec = adb_ro_blob(ndx, ADBI_NDX_PKGNAME_SPEC);
	if (!APK_BLOB_IS_NULL(pkgname_spec)) {
		repo->pkgname_spec = *apk_atomize_dup(&db->atoms, pkgname_spec);
		repo->absolute_pkgname = apk_blob_contains(pkgname_spec, APK_BLOB_STRLIT("://")) >= 0;
	}

	adb_ro_obj(ndx, ADBI_NDX_PACKAGES, &pkgs);
	for (i = ADBI_FIRST; i <= adb_ra_num(&pkgs); i++) {
		adb_ro_obj(&pkgs, i, &pkginfo);
		apk_pkgtmpl_from_adb(&tmpl, &pkginfo);
		if (tmpl.id.alg == APK_DIGEST_NONE) {
			num_broken++;
			apk_pkgtmpl_reset(&tmpl);
			continue;
		}

		tmpl.pkg.repos |= BIT(ctx->repo);
		if (!apk_db_pkg_add(db, &tmpl)) {
			r = -APKE_ADB_SCHEMA;
			break;
		}
	}

	apk_pkgtmpl_free(&tmpl);
	if (num_broken) apk_warn(out, "Repository " BLOB_FMT " has %d packages without hash",
		BLOB_PRINTF(repo->url_index_printable), num_broken);
	return r;
}

static const struct apk_extract_ops extract_index = {
	.v2index = load_v2index,
	.v3index = load_v3index,
};

static int load_index(struct apk_database *db, struct apk_istream *is, int repo)
{
	struct apkindex_ctx ctx = {
		.db = db,
		.repo = repo,
	};
	if (IS_ERR(is)) return PTR_ERR(is);
	apk_extract_init(&ctx.ectx, db->ctx, &extract_index);
	return apk_extract(&ctx.ectx, is);
}

static bool is_index_stale(struct apk_database *db, struct apk_repository *repo)
{
	struct stat st;
	char cache_filename[NAME_MAX];
	int cache_fd;

	if (!db->autoupdate) return false;
	if (!repo->is_remote) return false;
	if (!db->ctx->cache_max_age) return true;
	if (db->ctx->force & APK_FORCE_REFRESH) return true;
	if (apk_repo_index_cache_url(db, repo, &cache_fd, cache_filename, sizeof cache_filename) < 0) return true;
	if (fstatat(cache_fd, cache_filename, &st, 0) != 0) return true;
	repo->mtime = st.st_mtime;
	return (time(NULL) - st.st_mtime) > db->ctx->cache_max_age;
}

static int add_repository_component(struct apk_repoparser *rp, apk_blob_t url, const char *index_file, apk_blob_t tag)
{
	struct apk_database *db = container_of(rp, struct apk_database, repoparser);
	struct apk_repository *repo;
	apk_blob_t url_base, url_index, url_printable, url_index_printable;
	apk_blob_t pkgname_spec, dot = APK_BLOB_STRLIT(".");
	char buf[PATH_MAX];
	int tag_id = apk_db_get_tag_id(db, tag);

	if (index_file) {
		url_base = apk_blob_trim_end(url, '/');
		url_index = apk_blob_fmt(buf, sizeof buf, BLOB_FMT "/" BLOB_FMT "/%s",
			BLOB_PRINTF(url_base),
			BLOB_PRINTF(*db->arches->item[0]),
			index_file);
		url_base = APK_BLOB_PTR_LEN(url_index.ptr, url_base.len);
		url_printable = url_base;
		pkgname_spec = db->ctx->default_reponame_spec;
	} else {
		if (!apk_blob_rsplit(url, '/', &url_base, NULL)) url_base = dot;
		url_index = url;
		url_printable = url;
		pkgname_spec = db->ctx->default_pkgname_spec;
	}

	for (repo = &db->repos[0]; repo < &db->repos[db->num_repos]; repo++) {
		if (apk_blob_compare(url_base, repo->url_base) != 0) continue;
		if (apk_blob_compare(url_index, repo->url_index) != 0) continue;
		repo->tag_mask |= BIT(tag_id);
		return 0;
	}
	url_index = apk_balloc_dup(&db->ctx->ba, url_index);
	url_index_printable = apk_url_sanitize(url_index, &db->ctx->ba);
	if (url_base.ptr != dot.ptr) {
		// url base is a prefix of url index
		url_base = APK_BLOB_PTR_LEN(url_index.ptr, url_base.len);
	}
	url_printable = APK_BLOB_PTR_LEN(url_index_printable.ptr,
		url_index_printable.len + (url_printable.len - url_index.len));

	if (db->num_repos >= APK_MAX_REPOS) return -1;
	repo = &db->repos[db->num_repos++];
	*repo = (struct apk_repository) {
		.url_base = url_base,
		.url_printable = url_printable,
		.url_index = url_index,
		.url_index_printable = url_index_printable,
		.pkgname_spec = pkgname_spec,
		.is_remote = apk_url_local_file(url_index.ptr, url_index.len) == NULL ||
			apk_blob_starts_with(url_index, APK_BLOB_STRLIT("test:")),
		.tag_mask = BIT(tag_id),
		.v2_allowed = !apk_blob_ends_with(url_index, APK_BLOB_STRLIT(".adb")),
	};
	apk_digest_calc(&repo->hash, APK_DIGEST_SHA256, url_index.ptr, url_index.len);
	if (is_index_stale(db, repo)) repo->stale = 1;
	return 0;
}

static const struct apk_repoparser_ops db_repoparser_ops = {
	.repository = add_repository_component,
};

static void open_repository(struct apk_database *db, int repo_num)
{
	struct apk_out *out = &db->ctx->out;
	struct apk_repository *repo = &db->repos[repo_num];
	const char *error_action = "constructing url";
	unsigned int repo_mask = BIT(repo_num);
	unsigned int available_repos = 0;
	char open_url[PATH_MAX];
	int r, update_error = 0, open_fd = AT_FDCWD;

	error_action = "opening";
	if (!(db->ctx->flags & APK_NO_NETWORK)) available_repos = repo_mask;

	if (repo->is_remote && !(db->ctx->flags & APK_NO_CACHE)) {
		error_action = "opening from cache";
		if (repo->stale) {
			update_error = apk_cache_download(db, repo, NULL, NULL);
			switch (update_error) {
			case 0:
				db->repositories.updated++;
				// Fallthrough
			case -APKE_FILE_UNCHANGED:
				update_error = 0;
				repo->stale = 0;
				break;
			}
		}
		r = apk_repo_index_cache_url(db, repo, &open_fd, open_url, sizeof open_url);
	} else {
		if (repo->is_remote) {
			error_action = "fetching";
			apk_out_progress_note(out, "fetch " BLOB_FMT, BLOB_PRINTF(repo->url_index_printable));
		} else {
			available_repos = repo_mask;
			db->local_repos |= repo_mask;
		}
		r = apk_fmt(open_url, sizeof open_url, BLOB_FMT, BLOB_PRINTF(repo->url_index));
	}
	if (r < 0) goto err;
	r = load_index(db, apk_istream_from_fd_url(open_fd, open_url, apk_db_url_since(db, 0)), repo_num);
err:
	if (r || update_error) {
		if (repo->is_remote) {
			if (r) db->repositories.unavailable++;
			else db->repositories.stale++;
		}
		if (update_error)
			error_action = r ? "updating and opening" : "updating";
		else
			update_error = r;
		apk_warn(out, "%s " BLOB_FMT ": %s",
			error_action, BLOB_PRINTF(repo->url_index_printable), apk_error_str(update_error));
	}
	if (r == 0) {
		repo->available = 1;
		db->available_repos |= available_repos;
		for (unsigned int tag_id = 0, mask = repo->tag_mask; mask; mask >>= 1, tag_id++)
			if (mask & 1) db->repo_tags[tag_id].allowed_repos |= repo_mask;
	}
}

static int add_repository(struct apk_database *db, apk_blob_t line)
{
	return apk_repoparser_parse(&db->repoparser, line, true);
}

static int add_repos_from_file(void *ctx, int dirfd, const char *path, const char *file)
{
	struct apk_database *db = (struct apk_database *) ctx;
	struct apk_out *out = &db->ctx->out;
	int r;

	apk_repoparser_set_file(&db->repoparser, file);
	r = apk_db_parse_istream(db, apk_istream_from_file(dirfd, file), add_repository);
	if (r != 0) {
		if (dirfd != AT_FDCWD) return 0;
		apk_err(out, "failed to read repositories: %s: %s", file, apk_error_str(r));
		return r;
	}
	return 0;
}

static void setup_cache_repository(struct apk_database *db, apk_blob_t cache_dir)
{
	db->filename_repository = (struct apk_repository) {};
	db->cache_repository = (struct apk_repository) {
		.url_base = cache_dir,
		.url_printable = cache_dir,
		.pkgname_spec = db->ctx->default_cachename_spec,
		.absolute_pkgname = 1,
	};
	db->num_repo_tags = 1;
}

static int apk_db_name_rdepends(apk_hash_item item, void *pctx)
{
	struct apk_name *name = item, *rname;
	struct apk_name *touched[128];
	unsigned num_touched = 0;

	apk_array_foreach(p, name->providers) {
		apk_array_foreach(dep, p->pkg->depends) {
			rname = dep->name;
			rname->is_dependency |= !apk_dep_conflict(dep);
			if (!(rname->state_int & 1)) {
				if (!rname->state_int) {
					if (num_touched < ARRAY_SIZE(touched))
						touched[num_touched] = rname;
					num_touched++;
				}
				rname->state_int |= 1;
				apk_name_array_add(&rname->rdepends, name);
			}
		}
		apk_array_foreach(dep, p->pkg->install_if) {
			rname = dep->name;
			if (!(rname->state_int & 2)) {
				if (!rname->state_int) {
					if (num_touched < ARRAY_SIZE(touched))
						touched[num_touched] = rname;
					num_touched++;
				}
				rname->state_int |= 2;
				apk_name_array_add(&rname->rinstall_if, name);
			}
		}
	}

	if (num_touched > ARRAY_SIZE(touched)) {
		apk_array_foreach(p, name->providers) {
			apk_array_foreach(dep, p->pkg->depends)
				dep->name->state_int = 0;
			apk_array_foreach(dep, p->pkg->install_if)
				dep->name->state_int = 0;
		}
	} else for (unsigned i = 0; i < num_touched; i++)
		touched[i]->state_int = 0;

	return 0;
}

#ifdef __linux__
static int write_file(const char *fn, const char *fmt, ...)
{
	char buf[256];
	int n, fd, ret = -1;
	va_list va;

	fd  = open(fn, O_WRONLY);
	if (fd >= 0) {
		va_start(va, fmt);
		n = vsnprintf(buf, sizeof buf, fmt, va);
		va_end(va);
		if (write(fd, buf, n) == n) ret = 0;
		close(fd);
	}
	return ret;
}

static bool memfd_exec_check(void)
{
	char val[8];
	bool ret = false;
	int fd = open("/proc/sys/vm/memfd_noexec", O_RDONLY);
	if (fd >= 0) {
		if (read(fd, val, sizeof val) >= 1 && val[0] < '2') ret = true;
		close(fd);
	}
	return ret;
}

static bool unshare_check(void)
{
	int status;

	if (unshare(0) < 0) return false;
	pid_t pid = fork();
	if (pid == -1) return false;
	if (pid == 0) _Exit(unshare(CLONE_NEWNS) < 0 ? 1 : 0);
	while (waitpid(pid, &status, 0) < 0 && errno == EINTR);
	return WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

static int unshare_mount_namespace(struct apk_database *db)
{
	if (db->usermode) {
		uid_t uid = getuid();
		gid_t gid = getgid();
		if (unshare(CLONE_NEWNS | CLONE_NEWUSER) != 0) return -1;
		if (write_file("/proc/self/uid_map", "0 %d 1", uid) != 0) return -1;
		if (write_file("/proc/self/setgroups", "deny") != 0) return -1;
		if (write_file("/proc/self/gid_map", "0 %d 1", gid) != 0) return -1;
	} else {
		// if unshare fails as root, we continue with chroot
		if (unshare(CLONE_NEWNS) != 0) return 0;
	}
	if (mount("none", "/", NULL, MS_REC|MS_PRIVATE, NULL) != 0) return -1;
	// Create /proc and /dev in the chroot if needed
	if (!db->root_proc_ok) {
		mkdir("proc", 0755);
		if (mount("/proc", "proc", NULL, MS_BIND|MS_REC, NULL) < 0)
			mount("proc", "proc", "proc", 0, NULL);
	}
	if (!db->root_dev_ok) {
		mkdir("dev", 0755);
		mount("/dev", "dev", NULL, MS_BIND|MS_REC|MS_RDONLY, NULL);
	}
	return 0;
}

static int detect_tmpfs(int fd)
{
	struct statfs stfs;

	return fstatfs(fd, &stfs) == 0 && stfs.f_type == TMPFS_MAGIC;
}

static unsigned long map_statfs_flags(unsigned long f_flag)
{
	unsigned long mnt_flags = 0;
	if (f_flag & ST_RDONLY) mnt_flags |= MS_RDONLY;
	if (f_flag & ST_NOSUID) mnt_flags |= MS_NOSUID;
	if (f_flag & ST_NODEV)  mnt_flags |= MS_NODEV;
	if (f_flag & ST_NOEXEC) mnt_flags |= MS_NOEXEC;
	if (f_flag & ST_NOATIME) mnt_flags |= MS_NOATIME;
	if (f_flag & ST_NODIRATIME)mnt_flags |= MS_NODIRATIME;
#ifdef ST_RELATIME
	if (f_flag & ST_RELATIME) mnt_flags |= MS_RELATIME;
#endif
	if (f_flag & ST_SYNCHRONOUS) mnt_flags |= MS_SYNCHRONOUS;
	if (f_flag & ST_MANDLOCK) mnt_flags |= ST_MANDLOCK;
	return mnt_flags;
}

static char *find_mountpoint(int atfd, const char *rel_path)
{
	struct mntent *me;
	struct stat st;
	FILE *f;
	char *ret = NULL;
	dev_t dev;

	if (fstatat(atfd, rel_path, &st, 0) != 0)
		return NULL;
	dev = st.st_dev;

	f = setmntent("/proc/mounts", "r");
	if (f == NULL)
		return NULL;
	while ((me = getmntent(f)) != NULL) {
		if (strcmp(me->mnt_fsname, "rootfs") == 0)
			continue;
		if (fstatat(atfd, me->mnt_dir, &st, 0) == 0 &&
		    st.st_dev == dev) {
			ret = strdup(me->mnt_dir);
			break;
		}
	}
	endmntent(f);

	return ret;
}

static int remount_cache_rw(struct apk_database *db)
{
	struct apk_ctx *ac = db->ctx;
	struct apk_out *out = &ac->out;
	struct statfs stfs;

	if (fstatfs(db->cache_fd, &stfs) != 0) return -errno;

	db->cache_remount_flags = map_statfs_flags(stfs.f_flags);
	if ((ac->open_flags & (APK_OPENF_WRITE | APK_OPENF_CACHE_WRITE)) == 0) return 0;
	if ((db->cache_remount_flags & MS_RDONLY) == 0) return 0;

	/* remount cache read/write */
	db->cache_remount_dir = find_mountpoint(db->root_fd, db->cache_dir);
	if (db->cache_remount_dir == NULL) {
		apk_warn(out, "Unable to find cache directory mount point");
		return 0;
	}
	if (mount(0, db->cache_remount_dir, 0, MS_REMOUNT | (db->cache_remount_flags & ~MS_RDONLY), 0) != 0) {
		free(db->cache_remount_dir);
		db->cache_remount_dir = NULL;
		return -EROFS;
	}
	return 0;
}

static void remount_cache_ro(struct apk_database *db)
{
	if (!db->cache_remount_dir) return;
	mount(0, db->cache_remount_dir, 0, MS_REMOUNT | db->cache_remount_flags, 0);
	free(db->cache_remount_dir);
	db->cache_remount_dir = NULL;
}
#else
static bool memfd_exec_check(void) { return false; }
static bool unshare_check(void) { return false; }
static int unshare_mount_namespace(struct apk_database *db) { return 0; }
static int detect_tmpfs(int fd) { return 0; }
static int remount_cache_rw(struct apk_database *db) { return 0; }
static void remount_cache_ro(struct apk_database *db) { }
#endif

static int setup_cache(struct apk_database *db)
{
	db->cache_dir = db->ctx->cache_dir;
	db->cache_fd = openat(db->root_fd, db->cache_dir, O_DIRECTORY | O_RDONLY | O_CLOEXEC);
	if (db->cache_fd >= 0) {
		db->ctx->cache_packages = 1;
		return remount_cache_rw(db);
	}
	if (db->ctx->cache_dir_set || errno != ENOENT) return -errno;

	// The default cache does not exists, fallback to static cache directory
	db->cache_dir = apk_static_cache_dir;
	db->cache_fd = openat(db->root_fd, db->cache_dir, O_DIRECTORY | O_RDONLY | O_CLOEXEC);
	if (db->cache_fd < 0) {
		apk_make_dirs(db->root_fd, db->cache_dir, 0755, 0755);
		db->cache_fd = openat(db->root_fd, db->cache_dir, O_DIRECTORY | O_RDONLY | O_CLOEXEC);
		if (db->cache_fd < 0) {
			if (db->ctx->open_flags & APK_OPENF_WRITE) return -EROFS;
			db->cache_fd = -APKE_CACHE_NOT_AVAILABLE;
		}
	}
	return 0;
}

const char *apk_db_layer_name(int layer)
{
	switch (layer) {
	case APK_DB_LAYER_ROOT: return "lib/apk/db";
	case APK_DB_LAYER_UVOL: return "lib/apk/db-uvol";
	default:
		assert(!"invalid layer");
		return 0;
	}
}

#ifdef APK_UVOL_DB_TARGET
static void setup_uvol_target(struct apk_database *db)
{
	const struct apk_ctx *ac = db->ctx;
	const char *uvol_db = apk_db_layer_name(APK_DB_LAYER_UVOL);
	const char *uvol_target = APK_UVOL_DB_TARGET;
	const char *uvol_symlink_target = "../../" APK_UVOL_DB_TARGET;

	if (!(ac->open_flags & (APK_OPENF_WRITE|APK_OPENF_CREATE))) return;
	if (IS_ERR(ac->uvol)) return;
	if (faccessat(db->root_fd, uvol_db, F_OK, 0) == 0) return;
	if (faccessat(db->root_fd, uvol_target, F_OK, 0) != 0) return;

	// Create symlink from uvol_db to uvol_target in relative form
	symlinkat(uvol_symlink_target, db->root_fd, uvol_db);
}
#else
static void setup_uvol_target(struct apk_database *db) { }
#endif

void apk_db_init(struct apk_database *db, struct apk_ctx *ac)
{
	memset(db, 0, sizeof(*db));
	db->ctx = ac;
	apk_balloc_init(&db->ba_names, (sizeof(struct apk_name) + 16) * 256);
	apk_balloc_init(&db->ba_pkgs, sizeof(struct apk_package) * 256);
	apk_balloc_init(&db->ba_deps, sizeof(struct apk_dependency) * 256);
	apk_balloc_init(&db->ba_files, (sizeof(struct apk_db_file) + 32) * 256);
	apk_hash_init(&db->available.names, &pkg_name_hash_ops, 20000);
	apk_hash_init(&db->available.packages, &pkg_info_hash_ops, 10000);
	apk_hash_init(&db->installed.dirs, &dir_hash_ops, 20000);
	apk_hash_init(&db->installed.files, &file_hash_ops, 200000);
	apk_atom_init(&db->atoms, &db->ctx->ba);
	apk_dependency_array_init(&db->world);
	apk_pkgtmpl_init(&db->overlay_tmpl, db);
	apk_db_dir_instance_array_init(&db->ic.diris);
	apk_db_file_array_init(&db->ic.files);
	apk_protected_path_array_init(&db->ic.ppaths);
	list_init(&db->installed.packages);
	list_init(&db->installed.triggers);
	apk_protected_path_array_init(&db->protected_paths);
	apk_string_array_init(&db->filename_array);
	apk_blobptr_array_init(&db->arches);
	apk_name_array_init(&db->available.sorted_names);
	apk_package_array_init(&db->installed.sorted_packages);
	apk_repoparser_init(&db->repoparser, &ac->out, &db_repoparser_ops);
	db->root_fd = -1;
	db->lock_fd = -1;
	db->cache_fd = -APKE_CACHE_NOT_AVAILABLE;
	db->noarch = apk_atomize_dup(&db->atoms, APK_BLOB_STRLIT("noarch"));
}

int apk_db_open(struct apk_database *db)
{
	struct apk_ctx *ac = db->ctx;
	struct apk_out *out = &ac->out;
	const char *msg = NULL;
	int r = -1, i;

	apk_default_acl_dir = apk_db_acl_atomize(db, 0755, 0, 0);
	apk_default_acl_file = apk_db_acl_atomize(db, 0644, 0, 0);
	if (ac->open_flags == 0) {
		msg = "Invalid open flags (internal error)";
		goto ret_r;
	}
	if ((ac->open_flags & APK_OPENF_WRITE) &&
	    !(ac->open_flags & APK_OPENF_NO_AUTOUPDATE) &&
	    !(ac->flags & APK_NO_NETWORK))
		db->autoupdate = 1;

	setup_cache_repository(db, APK_BLOB_STR(ac->cache_dir));
	db->root_fd = apk_ctx_fd_root(ac);
	db->root_tmpfs = (ac->root_tmpfs == APK_AUTO) ? detect_tmpfs(db->root_fd) : ac->root_tmpfs;
	db->usermode = !!(ac->open_flags & APK_OPENF_USERMODE);

	if (!(ac->open_flags & APK_OPENF_CREATE)) {
		// Autodetect usermode from the installeddb owner
		struct stat st;
		if (fstatat(db->root_fd, apk_db_layer_name(APK_DB_LAYER_ROOT), &st, 0) == 0 &&
		    st.st_uid != 0)
			db->usermode = 1;
	}
	if (db->usermode) db->extract_flags |= APK_FSEXTRACTF_NO_CHOWN | APK_FSEXTRACTF_NO_SYS_XATTRS | APK_FSEXTRACTF_NO_DEVICES;

	setup_uvol_target(db);

	if (apk_array_len(ac->arch_list) && (ac->root_set || (ac->open_flags & APK_OPENF_ALLOW_ARCH))) {
		apk_array_foreach_item(arch, ac->arch_list)
			apk_db_add_arch(db, APK_BLOB_STR(arch));
		db->write_arch = ac->root_set;
	} else {
		struct apk_istream *is = apk_istream_from_file(db->root_fd, apk_arch_file);
		if (!IS_ERR(is)) apk_db_parse_istream(db, is, apk_db_add_arch);
	}
	if (apk_array_len(db->arches) == 0) {
		apk_db_add_arch(db, APK_BLOB_STR(APK_DEFAULT_ARCH));
		db->write_arch = 1;
	}
	apk_variable_set(&db->repoparser.variables, APK_BLOB_STRLIT("APK_ARCH"), *db->arches->item[0], APK_VARF_READONLY);

	// In usermode, unshare is need for chroot(2). Otherwise, it is needed
	// for new mount namespace to bind mount proc and dev from system root.
	if ((db->usermode || ac->root_set) && !(ac->flags & APK_NO_CHROOT)) {
		db->root_proc_ok = faccessat(db->root_fd, "proc/self", R_OK, 0) == 0;
		db->root_dev_ok = faccessat(db->root_fd, "dev/null", R_OK, 0) == 0;
		db->need_unshare = db->usermode || (!db->root_proc_ok || !db->root_dev_ok);

		// Check if unshare() works. It could be disabled, or seccomp filtered (docker).
		if (db->need_unshare && !db->usermode && !unshare_check()) {
			db->need_unshare = 0;
			db->memfd_failed = !db->root_proc_ok;
		}
	} else {
		db->root_proc_ok = access("/proc/self", R_OK) == 0;
		db->root_dev_ok = 1;
		db->memfd_failed = !db->root_proc_ok;
	}
	if (!db->memfd_failed) db->memfd_failed = !memfd_exec_check();

	db->id_cache = apk_ctx_get_id_cache(ac);

	if (ac->open_flags & APK_OPENF_WRITE) {
		msg = "Unable to lock database";
		db->lock_fd = openat(db->root_fd, apk_lock_file,
				     O_CREAT | O_RDWR | O_CLOEXEC, 0600);
		if (db->lock_fd < 0) {
			if (!(ac->open_flags & APK_OPENF_CREATE))
				goto ret_errno;
		} else if (flock(db->lock_fd, LOCK_EX | LOCK_NB) < 0) {
			struct sigaction sa, old_sa;

			if (!ac->lock_wait) goto ret_errno;

			apk_notice(out, "Waiting for repository lock");
			memset(&sa, 0, sizeof sa);
			sa.sa_handler = handle_alarm;
			sa.sa_flags   = SA_RESETHAND;
			sigaction(SIGALRM, &sa, &old_sa);

			alarm(ac->lock_wait);
			if (flock(db->lock_fd, LOCK_EX) < 0)
				goto ret_errno;

			alarm(0);
			sigaction(SIGALRM, &old_sa, NULL);
		}
	}

	if (ac->protected_paths) {
		apk_db_parse_istream(db, ac->protected_paths, apk_db_add_protected_path);
		ac->protected_paths = NULL;
	} else {
		apk_db_add_protected_path(db, APK_BLOB_STR("+etc"));
		apk_db_add_protected_path(db, APK_BLOB_STR("@etc/init.d"));
		apk_db_add_protected_path(db, APK_BLOB_STR("!etc/apk"));
		apk_dir_foreach_file(
			db->root_fd, "etc/apk/protected_paths.d",
			add_protected_paths_from_file, db,
			file_not_dot_list);
	}
	apk_protected_path_array_resize(&db->ic.ppaths, 0, apk_array_len(db->protected_paths));

	/* figure out where to have the cache */
	if (!(db->ctx->flags & APK_NO_CACHE)) {
		if ((r = setup_cache(db)) < 0) {
			msg = "Unable to setup the cache";
			goto ret_r;
		}
	}

	if (db->ctx->flags & APK_OVERLAY_FROM_STDIN) {
		db->ctx->flags &= ~APK_OVERLAY_FROM_STDIN;
		apk_db_read_overlay(db, apk_istream_from_fd(STDIN_FILENO));
	}

	if ((db->ctx->open_flags & APK_OPENF_NO_STATE) != APK_OPENF_NO_STATE) {
		for (i = 0; i < APK_DB_LAYER_NUM; i++) {
			r = apk_db_read_layer(db, i);
			if (r) {
				if (i != APK_DB_LAYER_ROOT) continue;
				if (!(r == -ENOENT && (ac->open_flags & APK_OPENF_CREATE))) {
					msg = "Unable to read database";
					goto ret_r;
				}
			}
			db->active_layers |= BIT(i);
		}
	} else {
		// Allow applets that use solver without state (fetch) to work correctly
		db->active_layers = ~0;
	}

	if (!(ac->open_flags & APK_OPENF_NO_INSTALLED_REPO)) {
		if (!apk_db_permanent(db) && apk_db_cache_active(db)) {
			apk_db_index_read(db, apk_istream_from_file(db->cache_fd, "installed"), APK_REPO_CACHE_INSTALLED);
		}
	}

	if (!(ac->open_flags & APK_OPENF_NO_CMDLINE_REPOS)) {
		apk_repoparser_set_file(&db->repoparser, "<command line>");
		apk_array_foreach_item(repo, ac->repository_list)
			apk_repoparser_parse(&db->repoparser, APK_BLOB_STR(repo), false);
		apk_array_foreach_item(config, ac->repository_config_list) {
			apk_blob_foreach_token(line, APK_BLOB_STR(config), APK_BLOB_STRLIT("\n"))
				apk_repoparser_parse(&db->repoparser, line, true);
		}
	}

	if (!(ac->open_flags & APK_OPENF_NO_SYS_REPOS)) {
		if (ac->repositories_file == NULL) {
			add_repos_from_file(db, db->root_fd, NULL, "etc/apk/repositories");
			apk_dir_foreach_config_file(db->root_fd,
				add_repos_from_file, db,
				file_not_dot_list,
				"etc/apk/repositories.d",
				"lib/apk/repositories.d",
				NULL);
		} else {
			add_repos_from_file(db, AT_FDCWD, NULL, ac->repositories_file);
		}
	}
	for (i = 0; i < db->num_repos; i++) open_repository(db, i);
	apk_out_progress_note(out, NULL);

	if (!(ac->open_flags & APK_OPENF_NO_SYS_REPOS) && db->repositories.updated > 0)
		apk_db_index_write_nr_cache(db);

	apk_hash_foreach(&db->available.names, apk_db_name_rdepends, db);

	if (apk_db_cache_active(db) && (ac->open_flags & (APK_OPENF_NO_REPOS|APK_OPENF_NO_INSTALLED)) == 0)
		apk_db_cache_foreach_item(db, mark_in_cache);

	db->open_complete = 1;

	if (db->compat_newfeatures) {
		apk_warn(out,
			"This apk-tools is OLD! Some packages %s.",
			db->compat_notinstallable ? "are not installable" : "might not function properly");
	}
	if (db->compat_depversions) {
		apk_warn(out,
			"The indexes contain broken packages which %s.",
			db->compat_notinstallable ? "are not installable" : "might not function properly");
	}

	ac->db = db;
	return 0;

ret_errno:
	r = -errno;
ret_r:
	if (msg != NULL)
		apk_err(out, "%s: %s", msg, apk_error_str(-r));
	apk_db_close(db);

	return r;
}

struct write_ctx {
	struct apk_database *db;
	int fd;
};

static int apk_db_write_layers(struct apk_database *db)
{
	struct layer_data {
		int fd;
		struct apk_ostream *installed, *scripts, *triggers;
	} layers[APK_DB_LAYER_NUM] = {0};
	struct apk_ostream *os;
	struct apk_package_array *pkgs;
	int i, r, rr = 0;

	for (i = 0; i < APK_DB_LAYER_NUM; i++) {
		struct layer_data *ld = &layers[i];
		if (!(db->active_layers & BIT(i))) {
			ld->fd = -1;
			continue;
		}

		ld->fd = openat(db->root_fd, apk_db_layer_name(i), O_DIRECTORY | O_RDONLY | O_CLOEXEC);
		if (ld->fd < 0) {
			if (i == APK_DB_LAYER_ROOT) return -errno;
			continue;
		}
		ld->installed = apk_ostream_to_file(ld->fd, "installed", 0644);
		ld->triggers  = apk_ostream_to_file(ld->fd, "triggers", 0644);
		if (db->scripts_tar) ld->scripts = apk_ostream_to_file(ld->fd, "scripts.tar", 0644);
		else ld->scripts = apk_ostream_gzip(apk_ostream_to_file(ld->fd, "scripts.tar.gz", 0644));

		if (i == APK_DB_LAYER_ROOT)
			os = apk_ostream_to_file(db->root_fd, apk_world_file, 0644);
		else
			os = apk_ostream_to_file(ld->fd, "world", 0644);
		if (IS_ERR(os)) {
			if (!rr) rr = PTR_ERR(os);
			continue;
		}
		apk_deps_write_layer(db, db->world, os, APK_BLOB_PTR_LEN("\n", 1), i);
		apk_ostream_write(os, "\n", 1);
		r = apk_ostream_close(os);
		if (!rr) rr = r;
	}

	pkgs = apk_db_sorted_installed_packages(db);
	apk_array_foreach_item(pkg, pkgs) {
		struct layer_data *ld = &layers[pkg->layer];
		if (ld->fd < 0) continue;
		apk_db_fdb_write(db, pkg->ipkg, ld->installed);
		apk_db_scriptdb_write(db, pkg->ipkg, ld->scripts);
		apk_db_triggers_write(db, pkg->ipkg, ld->triggers);
	}

	for (i = 0; i < APK_DB_LAYER_NUM; i++) {
		struct layer_data *ld = &layers[i];
		if (!(db->active_layers & BIT(i))) continue;

		if (!IS_ERR(ld->installed))
			r = apk_ostream_close(ld->installed);
		else	r = PTR_ERR(ld->installed);
		if (!rr) rr = r;

		if (!IS_ERR(ld->scripts)) {
			apk_tar_write_entry(ld->scripts, NULL, NULL);
			r = apk_ostream_close(ld->scripts);
		} else	r = PTR_ERR(ld->scripts);
		if (!rr) rr = r;

		if (!IS_ERR(ld->triggers))
			r = apk_ostream_close(ld->triggers);
		else	r = PTR_ERR(ld->triggers);
		if (!rr) rr = r;

		close(ld->fd);
	}
	return rr;
}

static int apk_db_write_arch(struct apk_database *db)
{
	struct apk_ostream *os;

	os = apk_ostream_to_file(db->root_fd, apk_arch_file, 0644);
	if (IS_ERR(os)) return PTR_ERR(os);

	apk_array_foreach_item(arch, db->arches) {
		apk_ostream_write(os, arch->ptr, arch->len);
		apk_ostream_write(os, "\n", 1);
	}
	return apk_ostream_close(os);
}

int apk_db_write_config(struct apk_database *db)
{
	struct apk_out *out = &db->ctx->out;
	int r, rr = 0;

	if ((db->ctx->flags & APK_SIMULATE) || db->ctx->root == NULL)
		return 0;

	if (db->ctx->open_flags & APK_OPENF_CREATE) {
		apk_make_dirs(db->root_fd, "lib/apk/db", 0755, 0755);
		apk_make_dirs(db->root_fd, "etc/apk", 0755, 0755);
	} else if (db->lock_fd < 0) {
		apk_err(out, "Refusing to write db without write lock!");
		return -1;
	}

	if (db->write_arch) {
		r = apk_db_write_arch(db);
		if (!rr) rr = r;
	}

	r = apk_db_write_layers(db);
	if (!rr) rr = r;

	r = apk_db_index_write_nr_cache(db);
	if (r < 0 && !rr) rr = r;

	if (rr) {
		apk_err(out, "System state may be inconsistent: failed to write database: %s",
			apk_error_str(rr));
	}
	return rr;
}

void apk_db_close(struct apk_database *db)
{
	struct apk_installed_package *ipkg, *ipkgn;

	list_for_each_entry_safe(ipkg, ipkgn, &db->installed.packages, installed_pkgs_list)
		apk_pkg_uninstall(NULL, ipkg->pkg);
	apk_protected_path_array_free(&db->protected_paths);
	apk_blobptr_array_free(&db->arches);
	apk_string_array_free(&db->filename_array);
	apk_pkgtmpl_free(&db->overlay_tmpl);
	apk_db_dir_instance_array_free(&db->ic.diris);
	apk_db_file_array_free(&db->ic.files);
	apk_protected_path_array_free(&db->ic.ppaths);
	apk_dependency_array_free(&db->world);

	apk_repoparser_free(&db->repoparser);
	apk_name_array_free(&db->available.sorted_names);
	apk_package_array_free(&db->installed.sorted_packages);
	apk_hash_free(&db->available.packages);
	apk_hash_free(&db->available.names);
	apk_hash_free(&db->installed.files);
	apk_hash_free(&db->installed.dirs);
	apk_atom_free(&db->atoms);
	apk_balloc_destroy(&db->ba_names);
	apk_balloc_destroy(&db->ba_pkgs);
	apk_balloc_destroy(&db->ba_files);
	apk_balloc_destroy(&db->ba_deps);

	remount_cache_ro(db);

	if (db->cache_fd >= 0) close(db->cache_fd);
	if (db->lock_fd >= 0) close(db->lock_fd);
}

int apk_db_get_tag_id(struct apk_database *db, apk_blob_t tag)
{
	int i;

	if (APK_BLOB_IS_NULL(tag))
		return APK_DEFAULT_REPOSITORY_TAG;

	if (tag.ptr[0] == '@') {
		for (i = 1; i < db->num_repo_tags; i++)
			if (apk_blob_compare(db->repo_tags[i].tag, tag) == 0)
				return i;
	} else {
		for (i = 1; i < db->num_repo_tags; i++)
			if (apk_blob_compare(db->repo_tags[i].plain_name, tag) == 0)
				return i;
	}
	if (i >= ARRAY_SIZE(db->repo_tags))
		return -1;

	db->num_repo_tags++;

	if (tag.ptr[0] == '@') {
		db->repo_tags[i].tag = *apk_atomize_dup(&db->atoms, tag);
	} else {
		char *tmp = alloca(tag.len + 1);
		tmp[0] = '@';
		memcpy(&tmp[1], tag.ptr, tag.len);
		db->repo_tags[i].tag = *apk_atomize_dup(&db->atoms, APK_BLOB_PTR_LEN(tmp, tag.len+1));
	}

	db->repo_tags[i].plain_name = db->repo_tags[i].tag;
	apk_blob_pull_char(&db->repo_tags[i].plain_name, '@');

	return i;
}

static int fire_triggers(apk_hash_item item, void *ctx)
{
	struct apk_database *db = (struct apk_database *) ctx;
	struct apk_db_dir *dbd = (struct apk_db_dir *) item;
	struct apk_installed_package *ipkg;
	int only_changed;

	list_for_each_entry(ipkg, &db->installed.triggers, trigger_pkgs_list) {
		if (!ipkg->run_all_triggers && !dbd->modified) continue;
		apk_array_foreach_item(trigger, ipkg->triggers) {
			only_changed = trigger[0] == '+';
			if (only_changed) ++trigger;
			if (trigger[0] != '/') continue;
			if (fnmatch(trigger, dbd->rooted_name, FNM_PATHNAME) != 0) continue;

			/* And place holder for script name */
			if (apk_array_len(ipkg->pending_triggers) == 0) {
				apk_string_array_add(&ipkg->pending_triggers, NULL);
				db->pending_triggers++;
			}
			if (!only_changed || dbd->modified)
				apk_string_array_add(&ipkg->pending_triggers, dbd->rooted_name);
			break;
		}
	}
	return 0;
}

int apk_db_fire_triggers(struct apk_database *db)
{
	apk_hash_foreach(&db->installed.dirs, fire_triggers, db);
	return db->pending_triggers;
}

static void script_panic(const char *reason)
{
	char buf[256];
	int n = apk_fmt(buf, sizeof buf, "%s: %s\n", reason, strerror(errno));
	apk_write_fully(STDERR_FILENO, buf, n);
	_exit(127);
}

struct env_buf {
	struct apk_string_array **arr;
	char data[1024];
	int pos;
};

static void env_buf_add(struct env_buf *enb, const char *key, const char *val)
{
	int n = snprintf(&enb->data[enb->pos], sizeof enb->data - enb->pos, "%s=%s", key, val);
	if (n >= sizeof enb->data - enb->pos) return;
	apk_string_array_add(enb->arr, &enb->data[enb->pos]);
	enb->pos += n + 1;
}

int apk_db_run_script(struct apk_database *db, const char *hook_type, const char *package_name, int fd, char **argv, const char *logpfx)
{
	struct env_buf enb;
	struct apk_ctx *ac = db->ctx;
	struct apk_out *out = &ac->out;
	struct apk_process p;
	int r, env_size_save = apk_array_len(ac->script_environment);
	char fd_path[NAME_MAX];
	const char *argv0 = apk_last_path_segment(argv[0]);
	const char *path = (fd < 0) ? argv[0] : apk_fmts(fd_path, sizeof fd_path, "/proc/self/fd/%d", fd);

	r = apk_process_init(&p, argv[0], logpfx, out, NULL);
	if (r != 0) {
		apk_err(out, "%s: process init: %s", argv0, apk_error_str(r));
		goto err;
	}

	enb.arr = &ac->script_environment;
	enb.pos = 0;
	env_buf_add(&enb, "APK_SCRIPT", hook_type);
	if (package_name) env_buf_add(&enb, "APK_PACKAGE", package_name);
	apk_string_array_add(&ac->script_environment, NULL);

	pid_t pid = apk_process_fork(&p);
	if (pid == -1) {
		r = -errno;
		apk_err(out, "%s: fork: %s", argv0, apk_error_str(r));
		goto err;
	}
	if (pid == 0) {
		umask(0022);
		if (fchdir(db->root_fd) != 0) script_panic("fchdir");
		if (!(ac->flags & APK_NO_CHROOT)) {
			if (db->need_unshare && unshare_mount_namespace(db) < 0) script_panic("unshare");
			if (ac->root_set && chroot(".") != 0) script_panic("chroot");
		}
		char **envp = &ac->script_environment->item[0];
		execve(path, argv, envp);
		script_panic("execve");
	}
	r = apk_process_run(&p);
err:
	apk_array_truncate(ac->script_environment, env_size_save);
	return r;
}

int apk_db_cache_active(struct apk_database *db)
{
	return db->cache_fd >= 0 && db->ctx->cache_packages;
}

struct foreach_cache_item_ctx {
	struct apk_database *db;
	apk_cache_item_cb cb;
	int static_cache;
};

static int foreach_cache_file(void *pctx, int dirfd, const char *path, const char *filename)
{
	struct foreach_cache_item_ctx *ctx = (struct foreach_cache_item_ctx *) pctx;
	struct apk_database *db = ctx->db;
	struct apk_file_info fi;

	if (apk_fileinfo_get(dirfd, filename, 0, &fi, NULL) == 0) {
		ctx->cb(db, ctx->static_cache, dirfd, filename,
			apk_db_get_pkg_by_name(db, APK_BLOB_STR(filename),
				fi.size, db->ctx->default_cachename_spec));
	}
	return 0;
}

int apk_db_cache_foreach_item(struct apk_database *db, apk_cache_item_cb cb)
{
	struct foreach_cache_item_ctx ctx = { .db = db, .cb = cb, .static_cache = true };
	struct stat st1, st2;

	int fd = openat(db->root_fd, apk_static_cache_dir, O_DIRECTORY | O_RDONLY | O_CLOEXEC);
	if (fd >= 0) {
		/* Do not handle static cache as static cache if the explicit
		 * cache is enabled at the static cache location */
		int r = 0;
		if (fstat(fd, &st1) == 0 && fstat(db->cache_fd, &st2) == 0 &&
		    (st1.st_dev != st2.st_dev || st1.st_ino != st2.st_ino))
			r = apk_dir_foreach_file(fd, NULL, foreach_cache_file, &ctx, NULL);
		close(fd);
		if (r) return r;
	}

	ctx.static_cache = false;
	if (db->cache_fd < 0) return db->cache_fd;
	return apk_dir_foreach_file(db->cache_fd, NULL, foreach_cache_file, &ctx, NULL);
}

int apk_db_permanent(struct apk_database *db)
{
	return !db->root_tmpfs;
}

int apk_db_check_world(struct apk_database *db, struct apk_dependency_array *world)
{
	struct apk_out *out = &db->ctx->out;
	int bad = 0, tag;

	if (db->ctx->force & APK_FORCE_BROKEN_WORLD) return 0;

	apk_array_foreach(dep, world) {
		tag = dep->repository_tag;
		if (tag == 0 || db->repo_tags[tag].allowed_repos != 0) continue;
		if (tag < 0) tag = 0;
		apk_warn(out, "The repository tag for world dependency '%s" BLOB_FMT "' does not exist",
			dep->name->name, BLOB_PRINTF(db->repo_tags[tag].tag));
		bad++;
	}

	return bad;
}

struct apk_package *apk_db_get_pkg(struct apk_database *db, struct apk_digest *id)
{
	if (id->len < APK_DIGEST_LENGTH_SHA1) return NULL;
	return apk_hash_get(&db->available.packages, APK_BLOB_PTR_LEN((char*)id->data, APK_DIGEST_LENGTH_SHA1));
}

struct apk_package *apk_db_get_pkg_by_name(struct apk_database *db, apk_blob_t filename, ssize_t filesize, apk_blob_t pkgname_spec)
{
	char buf[PATH_MAX];
	apk_blob_t name_format;
	struct apk_name *name;
	char split_char;
	int r;

	if (APK_BLOB_IS_NULL(pkgname_spec)) pkgname_spec = db->ctx->default_pkgname_spec;

	if (!apk_blob_rsplit(pkgname_spec, '/', NULL, &name_format)) name_format = pkgname_spec;
	if (!apk_blob_starts_with(name_format, APK_BLOB_STRLIT("${name}"))) return NULL;
	split_char = name_format.ptr[7];

	// if filename has path separator, assume full relative pkgname_spec
	if (apk_blob_chr(filename, '/')) name_format = pkgname_spec;

	// apk_pkg_subst_validate enforces pkgname_spec to be /${name} followed by [-._]
	// enumerate all potential names by walking the potential split points
	for (int i = 1; i < filename.len; i++) {
		if (filename.ptr[i] != split_char) continue;
		name = apk_db_get_name(db, APK_BLOB_PTR_LEN(filename.ptr, i));
		if (!name) continue;

		apk_array_foreach(p, name->providers) {
			struct apk_package *pkg = p->pkg;

			if (pkg->name != name) continue;
			if (pkg->size != filesize) continue;

			r = apk_blob_subst(buf, sizeof buf, name_format, apk_pkg_subst, pkg);
			if (r < 0) continue;

			if (apk_blob_compare(filename, APK_BLOB_PTR_LEN(buf, r)) == 0)
				return pkg;
		}
	}
	return NULL;
}

struct apk_package *apk_db_get_file_owner(struct apk_database *db,
					  apk_blob_t filename)
{
	struct apk_db_file *dbf;
	struct apk_db_file_hash_key key;

	filename = apk_blob_trim_start(filename, '/');
	if (!apk_blob_rsplit(filename, '/', &key.dirname, &key.filename)) {
		key.dirname = APK_BLOB_NULL;
		key.filename = filename;
	}
	dbf = (struct apk_db_file *) apk_hash_get(&db->installed.files, APK_BLOB_BUF(&key));
	if (dbf == NULL) return NULL;
	return dbf->diri->pkg;
}

unsigned int apk_db_get_pinning_mask_repos(struct apk_database *db, unsigned short pinning_mask)
{
	unsigned int repository_mask = 0;
	int i;

	for (i = 0; i < db->num_repo_tags && pinning_mask; i++) {
		if (!(BIT(i) & pinning_mask))
			continue;
		pinning_mask &= ~BIT(i);
		repository_mask |= db->repo_tags[i].allowed_repos;
	}
	return repository_mask;
}

struct apk_repository *apk_db_select_repo(struct apk_database *db,
					  struct apk_package *pkg)
{
	if (pkg->cached) return &db->cache_repository;
	if (pkg->filename_ndx) return &db->filename_repository;

	/* Pick first repository providing this package */
	unsigned int repos = pkg->repos & db->available_repos;
	if (repos == 0) return NULL;
	if (repos & db->local_repos) repos &= db->local_repos;
	for (int i = 0; i < APK_MAX_REPOS; i++) if (repos & BIT(i)) return &db->repos[i];
	return NULL;
}

int apk_db_index_read_file(struct apk_database *db, const char *file, int repo)
{
	return load_index(db, apk_istream_from_file(AT_FDCWD, file), repo);
}

int apk_db_repository_check(struct apk_database *db)
{
	if (db->ctx->force & APK_FORCE_MISSING_REPOSITORIES) return 0;
	if (!db->repositories.stale && !db->repositories.unavailable) return 0;
	apk_err(&db->ctx->out,
		"Not continuing due to stale/unavailable repositories. "
		"Use --force-missing-repositories to continue.");
	return -1;
}

struct install_ctx {
	struct apk_database *db;
	struct apk_package *pkg;
	struct apk_installed_package *ipkg;

	int script;
	char **script_args;
	unsigned int script_pending : 1;

	struct apk_extract_ctx ectx;

	uint64_t installed_size;
};

static void apk_db_run_pending_script(struct install_ctx *ctx)
{
	if (!ctx->script_pending) return;
	ctx->script_pending = false;
	apk_ipkg_run_script(ctx->ipkg, ctx->db, ctx->script, ctx->script_args);
}

static int read_info_line(void *_ctx, apk_blob_t line)
{
	struct install_ctx *ctx = (struct install_ctx *) _ctx;
	struct apk_installed_package *ipkg = ctx->ipkg;
	struct apk_database *db = ctx->db;
	apk_blob_t l, r;

	if (line.ptr == NULL || line.len < 1 || line.ptr[0] == '#')
		return 0;

	if (!apk_blob_split(line, APK_BLOB_STR(" = "), &l, &r))
		return 0;

	if (apk_blob_compare(APK_BLOB_STR("replaces"), l) == 0) {
		apk_blob_pull_deps(&r, db, &ipkg->replaces, false);
	} else if (apk_blob_compare(APK_BLOB_STR("replaces_priority"), l) == 0) {
		ipkg->replaces_priority = apk_blob_pull_uint(&r, 10);
	} else if (apk_blob_compare(APK_BLOB_STR("triggers"), l) == 0) {
		apk_array_truncate(ipkg->triggers, 0);
		apk_db_pkg_add_triggers(db, ctx->ipkg, r);
	} else {
		apk_extract_v2_control(&ctx->ectx, l, r);
	}
	return 0;
}

static int contains_control_character(const char *str)
{
	for (const uint8_t *p = (const uint8_t *) str; *p; p++) {
		if (*p < 0x20 || *p == 0x7f) return 1;
	}
	return 0;
}

static int apk_db_install_v2meta(struct apk_extract_ctx *ectx, struct apk_istream *is)
{
	struct install_ctx *ctx = container_of(ectx, struct install_ctx, ectx);
	apk_blob_t l, token = APK_BLOB_STR("\n");
	int r;

	apk_array_truncate(ctx->ipkg->replaces, 0);
	while (apk_istream_get_delim(is, token, &l) == 0) {
		r = read_info_line(ctx, l);
		if (r < 0) return r;
	}

	return 0;
}

static int apk_db_install_v3meta(struct apk_extract_ctx *ectx, struct adb_obj *pkg)
{
	static const int script_type_to_field[] = {
		[APK_SCRIPT_PRE_INSTALL]	= ADBI_SCRPT_PREINST,
		[APK_SCRIPT_POST_INSTALL]	= ADBI_SCRPT_POSTINST,
		[APK_SCRIPT_PRE_DEINSTALL]	= ADBI_SCRPT_PREDEINST,
		[APK_SCRIPT_POST_DEINSTALL]	= ADBI_SCRPT_POSTDEINST,
		[APK_SCRIPT_PRE_UPGRADE]	= ADBI_SCRPT_PREUPGRADE,
		[APK_SCRIPT_POST_UPGRADE]	= ADBI_SCRPT_POSTUPGRADE,
		[APK_SCRIPT_TRIGGER]		= ADBI_SCRPT_TRIGGER,
	};
	struct install_ctx *ctx = container_of(ectx, struct install_ctx, ectx);
	struct apk_database *db = ctx->db;
	struct apk_installed_package *ipkg = ctx->ipkg;
	struct adb_obj scripts, triggers, pkginfo, obj;
	int i;

	// Extract the information not available in index
	adb_ro_obj(pkg, ADBI_PKG_PKGINFO, &pkginfo);
	apk_deps_from_adb(&ipkg->replaces, db, adb_ro_obj(&pkginfo, ADBI_PI_REPLACES, &obj));
	ipkg->replaces_priority = adb_ro_int(pkg, ADBI_PKG_REPLACES_PRIORITY);
	ipkg->sha256_160 = 1;

	adb_ro_obj(pkg, ADBI_PKG_SCRIPTS, &scripts);
	for (i = 0; i < ARRAY_SIZE(script_type_to_field); i++) {
		apk_blob_t b = adb_ro_blob(&scripts, script_type_to_field[i]);
		if (APK_BLOB_IS_NULL(b)) continue;
		apk_ipkg_assign_script(ipkg, i, apk_blob_dup(b));
		ctx->script_pending |= (i == ctx->script);
	}

	adb_ro_obj(pkg, ADBI_PKG_TRIGGERS, &triggers);
	apk_string_array_resize(&ipkg->triggers, 0, adb_ra_num(&triggers));
	for (i = ADBI_FIRST; i <= adb_ra_num(&triggers); i++)
		apk_string_array_add(&ipkg->triggers, apk_blob_cstr(adb_ro_blob(&triggers, i)));
	if (apk_array_len(ctx->ipkg->triggers) != 0 && !list_hashed(&ipkg->trigger_pkgs_list))
		list_add_tail(&ipkg->trigger_pkgs_list, &db->installed.triggers);

	return 0;
}

static int apk_db_install_script(struct apk_extract_ctx *ectx, unsigned int type, uint64_t size, struct apk_istream *is)
{
	struct install_ctx *ctx = container_of(ectx, struct install_ctx, ectx);
	struct apk_package *pkg = ctx->pkg;

	apk_ipkg_add_script(pkg->ipkg, is, type, size);
	ctx->script_pending |= (type == ctx->script);
	return 0;
}

static int apk_db_install_file(struct apk_extract_ctx *ectx, const struct apk_file_info *ae, struct apk_istream *is)
{
	struct install_ctx *ctx = container_of(ectx, struct install_ctx, ectx);
	static const char dot1[] = "/./", dot2[] = "/../";
	struct apk_database *db = ctx->db;
	struct apk_ctx *ac = db->ctx;
	struct apk_out *out = &ac->out;
	struct apk_package *pkg = ctx->pkg, *opkg;
	struct apk_installed_package *ipkg = pkg->ipkg;
	struct apk_db_dir_instance *diri;
	apk_blob_t name = APK_BLOB_STR(ae->name), bdir, bfile;
	struct apk_db_file *file, *link_target_file = NULL;
	int ret = 0, r;

	apk_db_run_pending_script(ctx);

	/* Sanity check the file name */
	if (ae->name[0] == '/' || contains_control_character(ae->name) ||
	    strncmp(ae->name, &dot1[1], 2) == 0 ||
	    strncmp(ae->name, &dot2[1], 3) == 0 ||
	    strstr(ae->name, dot1) || strstr(ae->name, dot2)) {
		apk_warn(out, PKG_VER_FMT": ignoring malicious file %s",
			PKG_VER_PRINTF(pkg), ae->name);
		ipkg->broken_files = 1;
		return 0;
	}

	/* Installable entry */
	if (!S_ISDIR(ae->mode)) {
		if (!apk_blob_rsplit(name, '/', &bdir, &bfile)) {
			bdir = APK_BLOB_NULL;
			bfile = name;
		}

		/* Make sure the file is part of the cached directory tree */
		diri = apk_db_diri_query(db, bdir);
		if (diri == NULL) {
			if (!APK_BLOB_IS_NULL(bdir)) {
				apk_err(out, PKG_VER_FMT": "BLOB_FMT": no dirent in archive",
					PKG_VER_PRINTF(pkg), BLOB_PRINTF(name));
				ipkg->broken_files = 1;
				return 0;
			}
			diri = apk_db_diri_get(db, bdir, pkg);
		} else {
			diri = apk_db_diri_select(db, diri);
		}

		/* Check hard link target to exist in this package */
		if (S_ISREG(ae->mode) && ae->link_target) {
			link_target_file = apk_db_ipkg_find_file(db, APK_BLOB_STR(ae->link_target));
			if (!link_target_file) {
				apk_err(out, PKG_VER_FMT": "BLOB_FMT": no hard link target (%s) in archive",
					PKG_VER_PRINTF(pkg), BLOB_PRINTF(name), ae->link_target);
				ipkg->broken_files = 1;
				return 0;
			}
		}

		opkg = NULL;
		file = apk_db_file_query(db, bdir, bfile);
		if (file != NULL) {
			opkg = file->diri->pkg;
			switch (apk_pkg_replaces_file(opkg, pkg)) {
			case APK_PKG_REPLACES_CONFLICT:
				if (db->ctx->force & APK_FORCE_OVERWRITE) {
					apk_warn(out, PKG_VER_FMT": overwriting %s owned by "PKG_VER_FMT".",
						PKG_VER_PRINTF(pkg), ae->name, PKG_VER_PRINTF(opkg));
					break;
				}
				apk_err(out, PKG_VER_FMT": trying to overwrite %s owned by "PKG_VER_FMT".",
					PKG_VER_PRINTF(pkg), ae->name, PKG_VER_PRINTF(opkg));
				ipkg->broken_files = 1;
			case APK_PKG_REPLACES_NO:
				return 0;
			case APK_PKG_REPLACES_YES:
				break;
			}
		}

		if (opkg != pkg) {
			/* Create the file entry without adding it to hash */
			file = apk_db_file_new(db, diri, bfile);
		}

		apk_dbg2(out, "%s", ae->name);

		file->acl = apk_db_acl_atomize_digest(db, ae->mode, ae->uid, ae->gid, &ae->xattr_digest);
		r = apk_fs_extract(ac, ae, is, db->extract_flags, apk_pkg_ctx(pkg));
		if (r > 0) {
			char buf[APK_EXTRACTW_BUFSZ];
			if (r & APK_EXTRACTW_XATTR) ipkg->broken_xattr = 1;
			else ipkg->broken_files = 1;
			apk_warn(out, PKG_VER_FMT ": failed to preserve %s: %s",
				PKG_VER_PRINTF(pkg), ae->name, apk_extract_warning_str(r, buf, sizeof buf));
			r = 0;
		}
		switch (r) {
		case 0:
			// Hardlinks need special care for checksum
			if (!ipkg->sha256_160 && link_target_file)
				apk_dbf_digest_set(file, link_target_file->digest_alg, link_target_file->digest);
			else
				apk_dbf_digest_set(file, ae->digest.alg, ae->digest.data);

			if (ipkg->sha256_160 && S_ISLNK(ae->mode)) {
				struct apk_digest d;
				apk_digest_calc(&d, APK_DIGEST_SHA256_160,
						ae->link_target, strlen(ae->link_target));
				apk_dbf_digest_set(file, d.alg, d.data);
			} else if (file->digest_alg == APK_DIGEST_NONE && ae->digest.alg == APK_DIGEST_SHA256) {
				apk_dbf_digest_set(file, APK_DIGEST_SHA256_160, ae->digest.data);
			}
			break;
		case -APKE_NOT_EXTRACTED:
			file->broken = 1;
			break;
		case -ENOSPC:
			ret = r;
		case -APKE_UVOL_ROOT:
		case -APKE_UVOL_NOT_AVAILABLE:
		default:
			ipkg->broken_files = file->broken = 1;
			apk_err(out, PKG_VER_FMT ": failed to extract %s: %s",
				PKG_VER_PRINTF(pkg), ae->name, apk_error_str(r));
			break;
		}
	} else {
		struct apk_db_acl *expected_acl;

		apk_dbg2(out, "%s (dir)", ae->name);
		name = apk_blob_trim_end(name, '/');
		diri = apk_db_diri_get(db, name, pkg);
		diri->acl = apk_db_acl_atomize_digest(db, ae->mode, ae->uid, ae->gid, &ae->xattr_digest);
		expected_acl = diri->dir->owner ? diri->dir->owner->acl : NULL;
		apk_db_dir_apply_diri_permissions(db, diri);
		apk_db_dir_prepare(db, diri->dir, expected_acl, diri->dir->owner->acl);
	}
	ctx->installed_size += apk_calc_installed_size(ae->size);
	return ret;
}

static const struct apk_extract_ops extract_installer = {
	.v2meta = apk_db_install_v2meta,
	.v3meta = apk_db_install_v3meta,
	.script = apk_db_install_script,
	.file = apk_db_install_file,
};

static int apk_db_audit_file(struct apk_fsdir *d, apk_blob_t filename, struct apk_db_file *dbf)
{
	struct apk_file_info fi;
	int r, alg = APK_DIGEST_NONE;

	// Check file first
	if (dbf) alg = dbf->digest_alg;
	r = apk_fsdir_file_info(d, filename, APK_FI_NOFOLLOW | APK_FI_DIGEST(alg), &fi);
	if (r != 0 || alg == APK_DIGEST_NONE) return r != -ENOENT;
	if (apk_digest_cmp_blob(&fi.digest, alg, apk_dbf_digest_blob(dbf)) != 0) return 1;
	return 0;
}


struct fileid {
	dev_t dev;
	ino_t ino;
};
APK_ARRAY(fileid_array, struct fileid);

static bool fileid_get(struct apk_fsdir *fs, apk_blob_t filename, struct fileid *id)
{
	struct apk_file_info fi;
	if (apk_fsdir_file_info(fs, filename, APK_FI_NOFOLLOW, &fi) != 0) return false;
	*id = (struct fileid) {
		.dev = fi.data_device,
		.ino = fi.data_inode,
	};
	return true;
}

static int fileid_cmp(const void *a, const void *b)
{
	return memcmp(a, b, sizeof(struct fileid));
}

static void apk_db_purge_pkg(struct apk_database *db, struct apk_installed_package *ipkg, bool is_installed, struct fileid_array *fileids)
{
	struct apk_out *out = &db->ctx->out;
	struct apk_fsdir d;
	struct fileid id;
	int purge = db->ctx->flags & APK_PURGE;
	int ctrl = is_installed ? APK_FS_CTRL_DELETE : APK_FS_CTRL_CANCEL;

	if (fileids) {
		if (apk_array_len(fileids)) apk_array_qsort(fileids, fileid_cmp);
		else fileids = NULL;
	}

	apk_array_foreach_item(diri, ipkg->diris) {
		int dirclean = purge || !is_installed || apk_protect_mode_none(diri->dir->protect_mode);
		int delapknew = is_installed && !apk_protect_mode_none(diri->dir->protect_mode);
		apk_blob_t dirname = APK_BLOB_PTR_LEN(diri->dir->name, diri->dir->namelen);

		if (is_installed) diri->dir->modified = 1;
		apk_fsdir_get(&d, dirname, db->extract_flags, db->ctx, apk_pkg_ctx(ipkg->pkg));

		apk_array_foreach_item(file, diri->files) {
			if (file->audited) continue;
			struct apk_db_file_hash_key key = (struct apk_db_file_hash_key) {
				.dirname = dirname,
				.filename = APK_BLOB_PTR_LEN(file->name, file->namelen),
			};
			bool do_delete = !fileids || !fileid_get(&d, key.filename, &id) ||
				apk_array_bsearch(fileids, fileid_cmp, &id) == NULL;
			if (do_delete && (dirclean || apk_db_audit_file(&d, key.filename, file) == 0))
				apk_fsdir_file_control(&d, key.filename, ctrl);
			if (delapknew)
				apk_fsdir_file_control(&d, key.filename, APK_FS_CTRL_DELETE_APKNEW);
			apk_dbg2(out, DIR_FILE_FMT "%s", DIR_FILE_PRINTF(diri->dir, file), do_delete ? "" : " (not removing)");
			if (is_installed) {
				unsigned long hash = apk_blob_hash_seed(key.filename, diri->dir->hash);
				apk_hash_delete_hashed(&db->installed.files, APK_BLOB_BUF(&key), hash);
				db->installed.stats.files--;
			}
		}
		apk_db_diri_remove(db, diri);
	}
	apk_db_dir_instance_array_free(&ipkg->diris);
}

static uint8_t apk_db_migrate_files_for_priority(struct apk_database *db,
						 struct apk_installed_package *ipkg,
						 uint8_t priority,
						 struct fileid_array **fileids)
{
	struct apk_out *out = &db->ctx->out;
	struct apk_db_file *ofile;
	struct apk_db_file_hash_key key;
	struct apk_fsdir d;
	struct fileid id;
	unsigned long hash;
	int r, ctrl, inetc;
	uint8_t dir_priority, next_priority = APK_FS_PRIO_MAX;

	apk_array_foreach_item(diri, ipkg->diris) {
		struct apk_db_dir *dir = diri->dir;
		apk_blob_t dirname = APK_BLOB_PTR_LEN(dir->name, dir->namelen);

		apk_fsdir_get(&d, dirname, db->extract_flags, db->ctx, apk_pkg_ctx(ipkg->pkg));
		dir_priority = apk_fsdir_priority(&d);
		if (dir_priority != priority) {
			if (dir_priority > priority && dir_priority < next_priority)
				next_priority = dir_priority;
			continue;
		}
		// Used for passwd/group check later
		inetc = !apk_blob_compare(dirname, APK_BLOB_STRLIT("etc"));

		dir->modified = 1;
		apk_array_foreach_item(file, diri->files) {
			key = (struct apk_db_file_hash_key) {
				.dirname = dirname,
				.filename = APK_BLOB_PTR_LEN(file->name, file->namelen),
			};

			hash = apk_blob_hash_seed(key.filename, dir->hash);

			/* check for existing file */
			ofile = (struct apk_db_file *) apk_hash_get_hashed(
				&db->installed.files, APK_BLOB_BUF(&key), hash);

			if (!file->broken) {
				ctrl = APK_FS_CTRL_COMMIT;
				if (ofile && !ofile->diri->pkg) {
					// File was from overlay, delete the package's version
					ctrl = APK_FS_CTRL_CANCEL;
				} else if (!apk_protect_mode_none(diri->dir->protect_mode) &&
					   apk_db_audit_file(&d, key.filename, ofile) != 0) {
					// Protected directory, and a file without db entry
					// or with local modifications. Keep the filesystem file.
					// Determine if the package's file should be kept as .apk-new
					if ((db->ctx->flags & APK_CLEAN_PROTECTED) ||
					    apk_db_audit_file(&d, key.filename, file) == 0) {
						// No .apk-new files allowed, or the file on disk has the same
						// hash as the file from new package. Keep the on disk one.
						ctrl = APK_FS_CTRL_CANCEL;
					} else {
						// All files differ. Use the package's file as .apk-new.
						ctrl = APK_FS_CTRL_APKNEW;
						apk_msg(out, "  Installing file to " DIR_FILE_FMT "%s",
						    DIR_FILE_PRINTF(diri->dir, file),
						    db->ctx->apknew_suffix);
					}
				}

				// Commit changes
				r = apk_fsdir_file_control(&d, key.filename, ctrl);
				if (r < 0) {
					apk_err(out, PKG_VER_FMT": failed to commit " DIR_FILE_FMT ": %s",
						PKG_VER_PRINTF(ipkg->pkg),
						DIR_FILE_PRINTF(diri->dir, file),
						apk_error_str(r));
					ipkg->broken_files = 1;
				} else if (inetc && ctrl == APK_FS_CTRL_COMMIT) {
					// This is called when we successfully migrated the files
					// in the filesystem; we explicitly do not care about apk-new
					// or cancel cases, as that does not change the original file
					if (!apk_blob_compare(key.filename, APK_BLOB_STRLIT("passwd")) ||
					    !apk_blob_compare(key.filename, APK_BLOB_STRLIT("group"))) {
						// Reset the idcache because we have a new passwd/group
						apk_id_cache_reset(db->id_cache);
					}
				}
			}

			// Claim ownership of the file in db
			if (ofile == file) continue;
			if (ofile != NULL) {
				ofile->audited = 1;
				apk_hash_delete_hashed(&db->installed.files,
						       APK_BLOB_BUF(&key), hash);
			} else {
				if (fileids && fileid_get(&d, key.filename, &id))
					fileid_array_add(fileids, id);
				db->installed.stats.files++;
			}

			apk_hash_insert_hashed(&db->installed.files, file, hash);
		}
	}
	return next_priority;
}

static void apk_db_migrate_files(struct apk_database *db,
				 struct apk_installed_package *ipkg,
				 struct fileid_array **fileids)
{
	for (uint8_t prio = APK_FS_PRIO_DISK; prio != APK_FS_PRIO_MAX; )
		prio = apk_db_migrate_files_for_priority(db, ipkg, prio, fileids);
}

static int apk_db_unpack_pkg(struct apk_database *db,
			     struct apk_installed_package *ipkg,
			     int upgrade, struct apk_progress *prog,
			     char **script_args)
{
	struct apk_out *out = &db->ctx->out;
	struct install_ctx ctx;
	struct apk_progress_istream pis;
	struct apk_istream *is = NULL;
	struct apk_repository *repo;
	struct apk_package *pkg = ipkg->pkg;
	char file_url[PATH_MAX], cache_filename[NAME_MAX];
	int r, file_fd = AT_FDCWD, cache_fd = AT_FDCWD;
	bool need_copy = false;

	repo = apk_db_select_repo(db, pkg);
	if (repo == NULL) {
		r = -APKE_PACKAGE_NOT_FOUND;
		goto err_msg;
	}
	r = apk_repo_package_url(db, repo, pkg, &file_fd, file_url, sizeof file_url);
	if (r < 0) goto err_msg;
	if (apk_db_cache_active(db) && !pkg->cached && !(pkg->repos & db->local_repos)) need_copy = true;

	is = apk_istream_from_fd_url(file_fd, file_url, apk_db_url_since(db, 0));
	if (IS_ERR(is)) {
		r = PTR_ERR(is);
		if (r == -ENOENT && !pkg->filename_ndx)
			r = -APKE_INDEX_STALE;
		goto err_msg;
	}
	is = apk_progress_istream(&pis, is, prog);
	if (need_copy) {
		struct apk_istream *origis = is;
		r = apk_repo_package_url(db, &db->cache_repository, pkg, &cache_fd, cache_filename, sizeof cache_filename);
		if (r == 0)
			is = apk_istream_tee(is, apk_ostream_to_file_safe(cache_fd, cache_filename, 0644),
				APK_ISTREAM_TEE_COPY_META|APK_ISTREAM_TEE_OPTIONAL);
		if (is == origis)
			apk_warn(out, PKG_VER_FMT": unable to cache package",
				 PKG_VER_PRINTF(pkg));
	}

	ctx = (struct install_ctx) {
		.db = db,
		.pkg = pkg,
		.ipkg = ipkg,
		.script = upgrade ?
			APK_SCRIPT_PRE_UPGRADE : APK_SCRIPT_PRE_INSTALL,
		.script_args = script_args,
	};
	apk_extract_init(&ctx.ectx, db->ctx, &extract_installer);
	apk_extract_verify_identity(&ctx.ectx, pkg->digest_alg, apk_pkg_digest_blob(pkg));
	r = apk_extract(&ctx.ectx, is);
	if (need_copy && r == 0) pkg->cached = 1;
	if (r != 0) goto err_msg;
	apk_db_run_pending_script(&ctx);
	return 0;
err_msg:
	apk_err(out, PKG_VER_FMT": %s", PKG_VER_PRINTF(pkg), apk_error_str(r));
	return r;
}

int apk_db_install_pkg(struct apk_database *db, struct apk_package *oldpkg,
		       struct apk_package *newpkg, struct apk_progress *prog)
{
	char *script_args[] = { NULL, NULL, NULL, NULL };
	struct apk_installed_package *ipkg;
	struct fileid_array *fileids;
	int r = 0;

	fileid_array_init(&fileids);

	/* Upgrade script gets two args: <new-pkg> <old-pkg> */
	if (oldpkg != NULL && newpkg != NULL) {
		script_args[1] = apk_blob_cstr(*newpkg->version);
		script_args[2] = apk_blob_cstr(*oldpkg->version);
	} else {
		script_args[1] = apk_blob_cstr(*(oldpkg ? oldpkg->version : newpkg->version));
	}

	/* Just purging? */
	if (oldpkg != NULL && newpkg == NULL) {
		ipkg = oldpkg->ipkg;
		if (ipkg == NULL)
			goto ret_r;
		apk_ipkg_run_script(ipkg, db, APK_SCRIPT_PRE_DEINSTALL, script_args);
		apk_db_purge_pkg(db, ipkg, true, NULL);
		apk_ipkg_run_script(ipkg, db, APK_SCRIPT_POST_DEINSTALL, script_args);
		apk_pkg_uninstall(db, oldpkg);
		goto ret_r;
	}

	/* Install the new stuff */
	ipkg = apk_db_ipkg_create(db, newpkg);
	ipkg->run_all_triggers = 1;
	ipkg->broken_script = 0;
	ipkg->broken_files = 0;
	ipkg->broken_xattr = 0;
	if (apk_array_len(ipkg->triggers) != 0) {
		list_del(&ipkg->trigger_pkgs_list);
		list_init(&ipkg->trigger_pkgs_list);
		apk_array_foreach_item(trigger, ipkg->triggers) free(trigger);
		apk_array_truncate(ipkg->triggers, 0);
	}

	if (newpkg->installed_size != 0) {
		r = apk_db_unpack_pkg(db, ipkg, (oldpkg != NULL), prog, script_args);
		apk_db_ipkg_commit(db, ipkg);
		if (r != 0) {
			if (oldpkg != newpkg)
				apk_db_purge_pkg(db, ipkg, false, NULL);
			apk_pkg_uninstall(db, newpkg);
			goto ret_r;
		}
		apk_db_migrate_files(db, ipkg, oldpkg ? &fileids : NULL);
	}

	if (oldpkg != NULL && oldpkg != newpkg && oldpkg->ipkg != NULL) {
		apk_db_purge_pkg(db, oldpkg->ipkg, true, fileids);
		apk_pkg_uninstall(db, oldpkg);
	}

	apk_ipkg_run_script(
		ipkg, db,
		(oldpkg == NULL) ? APK_SCRIPT_POST_INSTALL : APK_SCRIPT_POST_UPGRADE,
		script_args);

	if (ipkg->broken_files || ipkg->broken_script)
		r = -1;
ret_r:
	free(script_args[1]);
	free(script_args[2]);
	fileid_array_free(&fileids);
	return r;
}

struct match_ctx {
	struct apk_database *db;
	struct apk_string_array *filter;
	apk_db_foreach_name_cb cb;
	void *cb_ctx;
};

static int apk_string_match(const char *str, struct apk_string_array *filter, const char **res)
{
	apk_array_foreach_item(match, filter) {
		if (fnmatch(match, str, FNM_CASEFOLD) == 0) {
			*res = match;
			return 1;
		}
	}
	return 0;
}

static int apk_name_match(struct apk_name *name, struct apk_string_array *filter, const char **res)
{
	if (!filter) {
		*res = NULL;
		return 1;
	}
	return apk_string_match(name->name, filter, res);
}

static int match_names(apk_hash_item item, void *pctx)
{
	struct match_ctx *ctx = (struct match_ctx *) pctx;
	struct apk_name *name = (struct apk_name *) item;
	const char *match;

	if (apk_name_match(name, ctx->filter, &match))
		return ctx->cb(ctx->db, match, name, ctx->cb_ctx);
	return 0;
}

int apk_db_foreach_matching_name(
	struct apk_database *db, struct apk_string_array *filter,
	apk_db_foreach_name_cb cb, void *ctx)
{
	struct apk_name *name;
	struct match_ctx mctx = {
		.db = db,
		.cb = cb,
		.cb_ctx = ctx,
	};
	int r;

	if (!filter || apk_array_len(filter) == 0) goto all;

	mctx.filter = filter;
	apk_array_foreach_item(match, filter)
		if (strchr(match, '*') != NULL)
			goto all;

	apk_array_foreach_item(match, filter) {
		name = (struct apk_name *) apk_hash_get(&db->available.names, APK_BLOB_STR(match));
		r = cb(db, match, name, ctx);
		if (r) return r;
	}
	return 0;

all:
	return apk_hash_foreach(&db->available.names, match_names, &mctx);
}

int apk_name_array_qsort(const void *a, const void *b)
{
	const struct apk_name * const* na = a, * const* nb = b;
	return apk_name_cmp_display(*na, *nb);
}

int apk_package_array_qsort(const void *a, const void *b)
{
	const struct apk_package * const* pa = a, * const* pb = b;
	return apk_pkg_cmp_display(*pa, *pb);
}

static int add_name(apk_hash_item item, void *ctx)
{
	struct apk_name_array **a = ctx;
	apk_name_array_add(a, (struct apk_name *) item);
	return 0;
}

struct apk_name_array *apk_db_sorted_names(struct apk_database *db)
{
	if (!db->sorted_names) {
		apk_name_array_resize(&db->available.sorted_names, 0, db->available.names.num_items);
		apk_hash_foreach(&db->available.names, add_name, &db->available.sorted_names);
		apk_array_qsort(db->available.sorted_names, apk_name_array_qsort);
		db->sorted_names = 1;
	}
	return db->available.sorted_names;
}

struct apk_package_array *apk_db_sorted_installed_packages(struct apk_database *db)
{
	struct apk_installed_package *ipkg;

	if (!db->sorted_installed_packages) {
		db->sorted_installed_packages = 1;
		apk_package_array_resize(&db->installed.sorted_packages, 0, db->installed.stats.packages);
		list_for_each_entry(ipkg, &db->installed.packages, installed_pkgs_list)
			apk_package_array_add(&db->installed.sorted_packages, ipkg->pkg);
		apk_array_qsort(db->installed.sorted_packages, apk_package_array_qsort);
	}
	return db->installed.sorted_packages;
}

int apk_db_foreach_sorted_name(struct apk_database *db, struct apk_string_array *filter,
			       apk_db_foreach_name_cb cb, void *cb_ctx)
{
	int r, walk_all = 0;
	struct apk_name *name;
	struct apk_name *results[128], **res;
	size_t i, num_res = 0;

	if (filter && apk_array_len(filter) != 0) {
		apk_array_foreach_item(match, filter) {
			name = (struct apk_name *) apk_hash_get(&db->available.names, APK_BLOB_STR(match));
			if (strchr(match, '*')) {
				walk_all = 1;
				continue;
			}
			if (!name) {
				cb(db, match, NULL, cb_ctx);
				continue;
			}
			if (walk_all) continue;
			if (num_res >= ARRAY_SIZE(results)) {
				walk_all = 1;
				continue;
			}
			results[num_res++] = name;
		}
	} else {
		filter = NULL;
		walk_all = 1;
	}

	if (walk_all) {
		struct apk_name_array *a = apk_db_sorted_names(db);
		res = a->item;
		num_res = apk_array_len(a);
	} else {
		qsort(results, num_res, sizeof results[0], apk_name_array_qsort);
		res = results;
	}

	for (i = 0; i < num_res; i++) {
		const char *match;
		name = res[i];
		if (apk_name_match(name, filter, &match)) {
			r = cb(db, match, name, cb_ctx);
			if (r) return r;
		}
	}
	return 0;
}
