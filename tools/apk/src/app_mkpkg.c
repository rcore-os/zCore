/* app_mkpkg.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2008-2021 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * This program is free software; you can redistribute it and/or modify it
 * under the terms of the GNU General Public License version 2 as published
 * by the Free Software Foundation. See http://www.gnu.org/ for details.
 */

#include <errno.h>
#include <stdio.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>

#include "apk_defines.h"
#include "apk_arch.h"
#include "apk_adb.h"
#include "apk_applet.h"
#include "apk_database.h"
#include "apk_pathbuilder.h"
#include "apk_extract.h"
#include "apk_balloc.h"
#include "apk_print.h"
#include "apk_xattr.h"

struct mkpkg_hardlink_key {
	dev_t device;
	ino_t inode;
};

struct mkpkg_hardlink {
	apk_hash_node hash_node;
	struct mkpkg_hardlink_key key;
	adb_val_t val;
};

static apk_blob_t mkpkg_hardlink_get_key(apk_hash_item item)
{
	struct mkpkg_hardlink *link = item;
	return APK_BLOB_STRUCT(link->key);
}

static const struct apk_hash_ops mkpkg_hardlink_hash_ops = {
	.node_offset = offsetof(struct mkpkg_hardlink, hash_node),
	.get_key = mkpkg_hardlink_get_key,
	.hash_key = apk_blob_hash,
	.compare = apk_blob_compare,
};

struct mkpkg_ctx {
	struct apk_ctx *ac;
	const char *files_dir, *output;
	struct apk_string_array *pathnames;
	struct adb db;
	struct adb_obj paths, files;
	struct apk_extract_ctx ectx;
	apk_blob_t package[ADBI_PKG_MAX];
	apk_blob_t info[ADBI_PI_MAX];
	apk_blob_t script[ADBI_SCRPT_MAX];
	struct apk_string_array *triggers;
	uint64_t installed_size;
	struct apk_pathbuilder pb;
	struct apk_hash link_by_inode;
	struct apk_balloc ba;
	int num_dirents;
	const char *compat;
	unsigned has_scripts : 1;
	unsigned output_stdout : 1;
	unsigned compat_rootnode : 1;
	unsigned compat_dirnode : 1;
	unsigned xattrs : 1;
};

#define MKPKG_OPTIONS(OPT) \
	OPT(OPT_MKPKG_compat,		APK_OPT_ARG "compat") \
	OPT(OPT_MKPKG_files,		APK_OPT_ARG APK_OPT_SH("F") "files") \
	OPT(OPT_MKPKG_info,		APK_OPT_ARG APK_OPT_SH("I") "info") \
	OPT(OPT_MKPKG_output,		APK_OPT_ARG APK_OPT_SH("o") "output") \
	OPT(OPT_MKPKG_rootnode,		APK_OPT_BOOL "rootnode") \
	OPT(OPT_MKPKG_script,		APK_OPT_ARG APK_OPT_SH("s") "script") \
	OPT(OPT_MKPKG_stdout,		"stdout") \
	OPT(OPT_MKPKG_trigger,		APK_OPT_ARG APK_OPT_SH("t") "trigger") \
	OPT(OPT_MKPKG_xattrs,		APK_OPT_BOOL "xattrs") \

APK_OPTIONS(mkpkg_options_desc, MKPKG_OPTIONS);

static int parse_info(struct mkpkg_ctx *ictx, struct apk_out *out, const char *optarg)
{
	apk_blob_t l, r;
	int i;

	if (!apk_blob_split(APK_BLOB_STR(optarg), APK_BLOB_STRLIT(":"), &l, &r)) {
		apk_err(out, "missing key or value: %s", optarg);
		return -EINVAL;
	}

	i = adb_s_field_by_name_blob(&schema_pkginfo, l);
	switch (i) {
	case 0:
		break;
	case ADBI_PI_FILE_SIZE:
	case ADBI_PI_INSTALLED_SIZE:
		goto inval;
	default:
		ictx->info[i] = r;
		return 0;
	}

	i = adb_s_field_by_name_blob(&schema_package, l);
	switch (i) {
	case ADBI_PKG_REPLACES_PRIORITY:
		ictx->package[i] = r;
		return 0;
	default:
		break;
	}

inval:
	apk_err(out, "invalid info field: " BLOB_FMT, BLOB_PRINTF(l));
	return -EINVAL;
}

static int mkpkg_parse_option(void *ctx, struct apk_ctx *ac, int optch, const char *optarg)
{
	struct apk_out *out = &ac->out;
	struct mkpkg_ctx *ictx = ctx;
	apk_blob_t l, r;
	int i, ret;

	switch (optch) {
	case APK_OPTIONS_INIT:
		apk_balloc_init(&ictx->ba, PATH_MAX * 256);
		apk_hash_init(&ictx->link_by_inode, &mkpkg_hardlink_hash_ops, 256);
		apk_string_array_init(&ictx->triggers);
		ictx->compat = "3.0.0_pre1";
		ictx->xattrs = 1;
		break;
	case OPT_MKPKG_compat:
		ictx->compat = optarg;
		break;
	case OPT_MKPKG_files:
		ictx->files_dir = optarg;
		break;
	case OPT_MKPKG_info:
		return parse_info(ictx, out, optarg);
	case OPT_MKPKG_output:
		ictx->output = optarg;
		break;
	case OPT_MKPKG_rootnode:
		ictx->compat = APK_OPTARG_VAL(optarg) ? "3.0.0_pre1" : "3.0.0_pre3";
		break;
	case OPT_MKPKG_script:
		if (!apk_blob_split(APK_BLOB_STR(optarg), APK_BLOB_STRLIT(":"), &l, &r)) {
			apk_err(out, "missing script type: %s", optarg);
			return -EINVAL;
		}
		i = adb_s_field_by_name_blob(&schema_scripts, l);
		if (!i) {
			apk_err(out, "invalid script type: " BLOB_FMT, BLOB_PRINTF(l));
			return -EINVAL;
		}
		ret = apk_blob_from_file(AT_FDCWD, r.ptr, &ictx->script[i]);
		if (ret) {
			apk_err(out, "failed to load script: " BLOB_FMT ": %s",
				BLOB_PRINTF(r), apk_error_str(ret));
			return ret;
		}
		ictx->has_scripts = 1;
		break;
	case OPT_MKPKG_stdout:
		ictx->output_stdout = 1;
		break;
	case OPT_MKPKG_trigger:
		apk_string_array_add(&ictx->triggers, (char*) optarg);
		break;
	case OPT_MKPKG_xattrs:
		ictx->xattrs = APK_OPTARG_VAL(optarg);
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static adb_val_t create_xattrs(struct adb *db, int fd)
{
	struct adb_obj xa;
	char names[1024], buf[1024];
	ssize_t len, vlen;
	adb_val_t val = ADB_NULL;
	int i;

	if (fd < 0) return ADB_NULL;
	len = apk_flistxattr(fd, names, sizeof names);
	if (len <= 0) goto done;

	adb_wo_alloca(&xa, &schema_xattr_array, db);
	for (i = 0; i < len; i += strlen(&names[i]) + 1) {
		vlen = apk_fgetxattr(fd, &names[i], buf, sizeof buf);
		if (vlen < 0) continue;

		apk_blob_t vec[] = {
			APK_BLOB_PTR_LEN(&names[i], strlen(&names[i])+1),
			APK_BLOB_PTR_LEN(buf, vlen),
		};
		adb_wa_append(&xa, adb_w_blob_vec(db, ARRAY_SIZE(vec), vec));
	}
	val = adb_w_arr(&xa);
	adb_wo_free(&xa);
done:
	close(fd);
	return val;
}

static int mkpkg_scan_dirent(void *pctx, int dirfd, const char *path, const char *entry)
{
	struct mkpkg_ctx *ctx = pctx;
	struct apk_file_info fi;
	int r;

	r = apk_fileinfo_get(dirfd, entry, APK_FI_NOFOLLOW, &fi, NULL);
	if (r) return r;
	if (!S_ISDIR(fi.mode)) return 0;

	int n = apk_pathbuilder_push(&ctx->pb, entry);
	apk_string_array_add(&ctx->pathnames, apk_balloc_cstr(&ctx->ba, apk_pathbuilder_get(&ctx->pb)));
	r = apk_dir_foreach_file_sorted(dirfd, entry, mkpkg_scan_dirent, ctx, NULL);
	apk_pathbuilder_pop(&ctx->pb, n);
	return r;
}

static adb_val_t hardlink_target(struct adb *db, const char *path, apk_blob_t file)
{
	uint16_t mode = htole16(S_IFREG);
	int n = 0;
	apk_blob_t vec[4];

	vec[n++] = APK_BLOB_STRUCT(mode);
	if (path[0]) {
		vec[n++] = APK_BLOB_STR(path);
		vec[n++] = APK_BLOB_STRLIT("/");
	}
	vec[n++] = file;
	return adb_w_blob_vec(db, n, vec);
}

static int mkpkg_process_dirent(void *pctx, int dirfd, const char *path, const char *entry)
{
	struct mkpkg_ctx *ctx = pctx;
	struct apk_ctx *ac = ctx->ac;
	struct apk_out *out = &ac->out;
	struct apk_id_cache *idc = apk_ctx_get_id_cache(ac);
	struct apk_file_info fi;
	struct adb_obj fio, acl;
	struct mkpkg_hardlink *link = NULL;
	struct mkpkg_hardlink_key key;
	apk_blob_t name = APK_BLOB_STR(entry), target = APK_BLOB_NULL;
	union {
		uint16_t mode;
		struct {
			uint16_t mode;
			uint64_t dev;
		} __attribute__((packed)) dev;
		struct {
			uint16_t mode;
			char target[1022];
		} symlink;
	} ft;
	int r, n;

	ctx->num_dirents++;
	r = apk_fileinfo_get(dirfd, entry, APK_FI_NOFOLLOW | APK_FI_DIGEST(APK_DIGEST_SHA256), &fi, NULL);
	if (r) return r;

	switch (fi.mode & S_IFMT) {
	case S_IFREG:
		key = (struct mkpkg_hardlink_key) {
			.device = fi.data_device,
			.inode = fi.data_inode,
		};
		if (fi.num_links > 1) {
			link = apk_hash_get(&ctx->link_by_inode, APK_BLOB_STRUCT(key));
			if (link) break;

			link = apk_balloc_new(&ctx->ba, struct mkpkg_hardlink);
			*link = (struct mkpkg_hardlink) {
				.key = key,
				.val = hardlink_target(&ctx->db, path, name),
			};
			apk_hash_insert(&ctx->link_by_inode, link);
			link = NULL;
		}
		ctx->installed_size += fi.size;
		break;
	case S_IFBLK:
	case S_IFCHR:
	case S_IFIFO:
		ft.dev.mode = htole16(fi.mode & S_IFMT);
		ft.dev.dev = htole64(fi.device);
		target = APK_BLOB_STRUCT(ft.dev);
		break;
	case S_IFLNK:
		ft.symlink.mode = htole16(fi.mode & S_IFMT);
		r = readlinkat(dirfd, entry, ft.symlink.target, sizeof ft.symlink.target);
		if (r < 0) return r;
		target = APK_BLOB_PTR_LEN((void*)&ft.symlink, sizeof(ft.symlink.mode) + r);
		r = 0;
		break;
	case S_IFDIR:
		// Processed from the main loop.
		return 0;
	default:
		n = apk_pathbuilder_push(&ctx->pb, entry);
		apk_out(out, "%s: special file ignored", apk_pathbuilder_cstr(&ctx->pb));
		apk_pathbuilder_pop(&ctx->pb, n);
		return 0;
	}

	adb_wo_alloca(&fio, &schema_file, &ctx->db);
	adb_wo_alloca(&acl, &schema_acl, &ctx->db);
	adb_wo_blob(&fio, ADBI_FI_NAME, name);
	if ((fi.mode & S_IFMT) == S_IFREG)
		adb_wo_blob(&fio, ADBI_FI_HASHES, APK_DIGEST_BLOB(fi.digest));
	if (!APK_BLOB_IS_NULL(target))
		adb_wo_blob(&fio, ADBI_FI_TARGET, target);
	else if (link)
		adb_wo_val(&fio, ADBI_FI_TARGET, link->val);
	adb_wo_int(&fio, ADBI_FI_MTIME, apk_get_build_time(fi.mtime));
	adb_wo_int(&fio, ADBI_FI_SIZE, fi.size);

	adb_wo_int(&acl, ADBI_ACL_MODE, fi.mode & 07777);
	adb_wo_blob(&acl, ADBI_ACL_USER, apk_id_cache_resolve_user(idc, fi.uid));
	adb_wo_blob(&acl, ADBI_ACL_GROUP, apk_id_cache_resolve_group(idc, fi.gid));
	if (ctx->xattrs)
		adb_wo_val(&acl, ADBI_ACL_XATTRS, create_xattrs(&ctx->db, openat(dirfd, entry, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)));
	adb_wo_obj(&fio, ADBI_FI_ACL, &acl);

	adb_wa_append_obj(&ctx->files, &fio);

	return r;
}

static int mkpkg_process_directory(struct mkpkg_ctx *ctx, int atfd, const char *entry)
{
	apk_blob_t root = APK_BLOB_STRLIT("root");
	struct apk_ctx *ac = ctx->ac;
	struct apk_id_cache *idc = apk_ctx_get_id_cache(ac);
	struct apk_out *out = &ac->out;
	struct adb_obj acl, fio;
	struct apk_file_info fi;
	const char *path = entry ?: ".";
	apk_blob_t dirname = APK_BLOB_STR(entry);
	int r;

	r = apk_fileinfo_get(atfd, path, APK_FI_NOFOLLOW, &fi, NULL);
	if (r) return r;

	ctx->num_dirents = 0;
	r = apk_dir_foreach_file_sorted(atfd, path, mkpkg_process_dirent, ctx, NULL);
	if (r) goto done;

	apk_blob_t user = apk_id_cache_resolve_user(idc, fi.uid);
	apk_blob_t group = apk_id_cache_resolve_group(idc, fi.gid);
	mode_t mode = fi.mode & ~S_IFMT;
	if (mode == 0755 && adb_ra_num(&ctx->files) == 0 && apk_blob_compare(user, root) == 0 && apk_blob_compare(group, root) == 0) {
		// Prune empty directory with default acl if possible
		if (!entry && !ctx->compat_rootnode) return 0;
		if ( entry && ctx->num_dirents && !ctx->compat_dirnode) return 0;
	}

	adb_wo_alloca(&fio, &schema_dir, &ctx->db);
	adb_wo_alloca(&acl, &schema_acl, &ctx->db);
	adb_wo_blob(&fio, ADBI_DI_NAME, dirname);
	adb_wo_int(&acl, ADBI_ACL_MODE, mode);
	adb_wo_blob(&acl, ADBI_ACL_USER, user);
	adb_wo_blob(&acl, ADBI_ACL_GROUP, group);
	if (ctx->xattrs)
		adb_wo_val(&acl, ADBI_ACL_XATTRS, create_xattrs(&ctx->db, openat(atfd, path, O_DIRECTORY | O_RDONLY | O_CLOEXEC)));
	adb_wo_obj(&fio, ADBI_DI_ACL, &acl);
	adb_wo_obj(&fio, ADBI_DI_FILES, &ctx->files);
	adb_wa_append_obj(&ctx->paths, &fio);
done:
	if (r) apk_err(out, "failed to process directory '%s': %d", apk_pathbuilder_cstr(&ctx->pb), r);
	return r;
}

static int check_required(struct apk_out *out, apk_blob_t *vals, int index, const struct adb_object_schema *schema)
{
	if (!APK_BLOB_IS_NULL(vals[index])) return 0;
	apk_err(out, "required info field '%s' not provided",
		schema->fields[index-1].name);
	return -EINVAL;
}

static int assign_fields(struct apk_out *out, apk_blob_t *vals, int num_vals, struct adb_obj *obj)
{
	int i, r;

	for (i = 0; i < num_vals; i++) {
		apk_blob_t b = vals[i];
		if (APK_BLOB_IS_NULL(b)) continue;

		adb_val_t val = adb_wo_val_fromstring(obj, i, b);
		if (ADB_IS_ERROR(val)) {
			r = ADB_VAL_VALUE(val);
			apk_err(out, "info field '%s' has invalid value: %s",
				obj->schema->fields[i-1].name, apk_error_str(r));
			return r;
		}
	}
	return 0;
}

static void mkpkg_setup_compat(struct mkpkg_ctx *ctx)
{
	static const char compat_versions[] = {
		"3.0.0_pre3\0"
		"3.0.0_rc9\0"
	};
	apk_blob_t compat_ver = APK_BLOB_STR(ctx->compat);
	int i = 0;

	for (const char *v = compat_versions; *v; v += strlen(v) + 1, i++)
		if (apk_version_compare(compat_ver, APK_BLOB_STR(v)) & APK_VERSION_LESS) break;

	switch (i) {
	case 0: ctx->compat_rootnode = 1; // fallthrough
	case 1: ctx->compat_dirnode = 1;  // fallthrough
	default:
		break;
	}
}

static int mkpkg_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_out *out = &ac->out;
	struct apk_trust *trust = apk_ctx_get_trust(ac);
	struct adb_obj pkg, pkgi;
	int i, j, r, dirfd = -1;
	struct mkpkg_ctx *ctx = pctx;
	struct apk_ostream *os;
	struct apk_digest d = {};
	char outbuf[NAME_MAX];
	const int uid_len = apk_digest_alg_len(APK_DIGEST_SHA1);
	apk_blob_t uid = APK_BLOB_PTR_LEN((char*)d.data, uid_len);

	ctx->ac = ac;
	mkpkg_setup_compat(ctx);
	apk_string_array_init(&ctx->pathnames);
	adb_w_init_alloca(&ctx->db, ADB_SCHEMA_PACKAGE, 40);
	adb_wo_alloca(&pkg, &schema_package, &ctx->db);
	adb_wo_alloca(&pkgi, &schema_pkginfo, &ctx->db);
	adb_wo_alloca(&ctx->paths, &schema_dir_array, &ctx->db);
	adb_wo_alloca(&ctx->files, &schema_file_array, &ctx->db);

	// prepare package info
	r = -EINVAL;
	if (check_required(out, ctx->info, ADBI_PI_NAME, &schema_pkginfo) ||
	    check_required(out, ctx->info, ADBI_PI_VERSION, &schema_pkginfo))
		goto err;

	if (APK_BLOB_IS_NULL(ctx->info[ADBI_PI_ARCH]))
		ctx->info[ADBI_PI_ARCH] = APK_BLOB_STRLIT(APK_DEFAULT_ARCH);

	r = assign_fields(out, ctx->info, ARRAY_SIZE(ctx->info), &pkgi);
	if (r) goto err;

	r = assign_fields(out, ctx->package, ARRAY_SIZE(ctx->package), &pkg);
	if (r) goto err;

	// scan and add all files
	if (ctx->files_dir) {
		dirfd = openat(AT_FDCWD, ctx->files_dir, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
		if (dirfd < 0) {
			r = -errno;
			apk_err(out, "file directory '%s': %s",
				ctx->files_dir, apk_error_str(r));
			goto err;
		}
		r = apk_dir_foreach_file_sorted(dirfd, NULL, mkpkg_scan_dirent, ctx, NULL);
		if (r) goto err;
		apk_array_qsort(ctx->pathnames, apk_string_array_qsort);

		r = mkpkg_process_directory(ctx, dirfd, NULL);
		if (r) goto err;
		apk_array_foreach_item(dir, ctx->pathnames) {
			r = mkpkg_process_directory(ctx, dirfd, dir);
			if (r) goto err;
		}
		if (!ctx->installed_size) ctx->installed_size = 1;
	}
	if (ctx->has_scripts && ctx->installed_size == 0) ctx->installed_size = 1;
	adb_wo_int(&pkgi, ADBI_PI_INSTALLED_SIZE, ctx->installed_size);
	adb_wo_blob_raw(&pkgi, ADBI_PI_HASHES, uid);

	adb_wo_obj(&pkg, ADBI_PKG_PKGINFO, &pkgi);
	adb_wo_obj(&pkg, ADBI_PKG_PATHS, &ctx->paths);
	if (ctx->has_scripts) {
		struct adb_obj scripts;
		adb_wo_alloca(&scripts, &schema_scripts, &ctx->db);
		for (i = ADBI_FIRST; i < ADBI_SCRPT_MAX; i++)
			adb_wo_blob(&scripts, i, ctx->script[i]);
		adb_wo_obj(&pkg, ADBI_PKG_SCRIPTS, &scripts);
	}
	if (ctx->triggers) {
		struct adb_obj triggers;
		adb_wo_alloca(&triggers, &schema_string_array, &ctx->db);
		apk_array_foreach_item(trigger, ctx->triggers)
			adb_wa_append_fromstring(&triggers, APK_BLOB_STR(trigger));
		adb_wo_obj(&pkg, ADBI_PKG_TRIGGERS, &triggers);
		adb_wo_free(&triggers);
	}
	adb_w_rootobj(&pkg);

	// re-read since object resets
	adb_r_rootobj(&ctx->db, &pkg, &schema_package);
	adb_ro_obj(&pkg, ADBI_PKG_PKGINFO, &pkgi);
	adb_ro_obj(&pkg, ADBI_PKG_PATHS, &ctx->paths);

	// fill in unique id
	apk_digest_calc(&d, APK_DIGEST_SHA256, ctx->db.adb.ptr, ctx->db.adb.len);
	uid = adb_ro_blob(&pkgi, ADBI_PI_HASHES);
	memcpy(uid.ptr, d.data, uid.len);

	if (ctx->output_stdout) {
		os = apk_ostream_to_fd(STDOUT_FILENO);
	} else {
		if (!ctx->output) {
			r = apk_blob_subst(outbuf, sizeof outbuf, ac->default_pkgname_spec, adb_s_field_subst, &pkgi);
			if (r < 0) goto err;
			ctx->output = outbuf;
		}
		os = apk_ostream_to_file(AT_FDCWD, ctx->output, 0644);
	}

	// construct package with ADB as header, and the file data in
	// concatenated data blocks
	os = adb_compress(os, &ac->compspec);
	if (IS_ERR(os)) {
		r = PTR_ERR(os);
		goto err;
	}

	adb_c_adb(os, &ctx->db, trust);
	if (ctx->files_dir) {
		int files_fd = openat(AT_FDCWD, ctx->files_dir, O_DIRECTORY | O_RDONLY | O_CLOEXEC);
		for (i = ADBI_FIRST; i <= adb_ra_num(&ctx->paths); i++) {
			struct adb_obj path, files, file;
			adb_ro_obj(&ctx->paths, i, &path);
			adb_ro_obj(&path, ADBI_DI_FILES, &files);
			apk_blob_t dirname = adb_ro_blob(&path, ADBI_DI_NAME);

			apk_pathbuilder_setb(&ctx->pb, dirname);
			for (j = ADBI_FIRST; j <= adb_ra_num(&files); j++) {
				adb_ro_obj(&files, j, &file);
				apk_blob_t filename = adb_ro_blob(&file, ADBI_FI_NAME);
				apk_blob_t target = adb_ro_blob(&file, ADBI_FI_TARGET);
				uint64_t sz = adb_ro_int(&file, ADBI_FI_SIZE);
				if (!APK_BLOB_IS_NULL(target)) continue;
				if (!sz) continue;
				struct adb_data_package hdr = {
					.path_idx = htole32(i),
					.file_idx = htole32(j),
				};
				int n = apk_pathbuilder_pushb(&ctx->pb, filename);
				adb_c_block_data(
					os, APK_BLOB_STRUCT(hdr), sz,
					apk_istream_from_fd(openat(files_fd,
						apk_pathbuilder_cstr(&ctx->pb),
						O_RDONLY | O_CLOEXEC)));
				apk_pathbuilder_pop(&ctx->pb, n);
			}
		}
		close(files_fd);
	}
	r = apk_ostream_close(os);

err:
	adb_wo_free(&ctx->paths);
	adb_free(&ctx->db);
	if (r) apk_err(out, "failed to create package: %s", apk_error_str(r));
	apk_string_array_free(&ctx->triggers);
	apk_hash_free(&ctx->link_by_inode);
	apk_string_array_free(&ctx->pathnames);
	apk_balloc_destroy(&ctx->ba);
	if (dirfd >= 0) close(dirfd);
	return r;
}

static struct apk_applet apk_mkpkg = {
	.name = "mkpkg",
	.options_desc = mkpkg_options_desc,
	.optgroup_generation = 1,
	.context_size = sizeof(struct mkpkg_ctx),
	.parse = mkpkg_parse_option,
	.main = mkpkg_main,
};

APK_DEFINE_APPLET(apk_mkpkg);
