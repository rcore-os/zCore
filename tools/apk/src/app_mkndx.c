/* app_mkndx.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2020 Timo Teräs <timo.teras@iki.fi>
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

#include "apk_adb.h"
#include "apk_applet.h"
#include "apk_database.h"
#include "apk_extract.h"
#include "apk_print.h"

struct mkndx_ctx {
	const char *index;
	const char *output;
	const char *description;
	apk_blob_t pkgname_spec;
	apk_blob_t filter_spec;

	apk_blob_t r;
	struct adb db;
	struct adb_obj pkgs;
	struct adb_obj pkginfo;
	uint8_t hash_alg;
	uint8_t pkgname_spec_set : 1;
	uint8_t filter_spec_set : 1;

	struct apk_extract_ctx ectx;
};

#define ALLOWED_HASH (BIT(APK_DIGEST_SHA256)|BIT(APK_DIGEST_SHA256_160))

#define MKNDX_OPTIONS(OPT) \
	OPT(OPT_MKNDX_description,	APK_OPT_ARG APK_OPT_SH("d") "description") \
	OPT(OPT_MKNDX_filter_spec,	APK_OPT_ARG "filter-spec") \
	OPT(OPT_MKNDX_hash,		APK_OPT_ARG "hash") \
	OPT(OPT_MKNDX_index,		APK_OPT_ARG APK_OPT_SH("x") "index") \
	OPT(OPT_MKNDX_output,		APK_OPT_ARG APK_OPT_SH("o") "output") \
	OPT(OPT_MKNDX_pkgname_spec,	APK_OPT_ARG "pkgname-spec") \
	OPT(OPT_MKNDX_rewrite_arch,	APK_OPT_ARG "rewrite-arch")

APK_OPTIONS(mkndx_options_desc, MKNDX_OPTIONS);

static int mkndx_parse_option(void *ctx, struct apk_ctx *ac, int optch, const char *optarg)
{
	struct mkndx_ctx *ictx = ctx;
	struct apk_out *out = &ac->out;

	switch (optch) {
	case APK_OPTIONS_INIT:
		ictx->hash_alg = APK_DIGEST_SHA256;
		ictx->pkgname_spec = ac->default_pkgname_spec;
		break;
	case OPT_MKNDX_description:
		ictx->description = optarg;
		break;
	case OPT_MKNDX_filter_spec:
		ictx->filter_spec = APK_BLOB_STR(optarg);
		ictx->filter_spec_set = 1;
		break;
	case OPT_MKNDX_hash:
		ictx->hash_alg = apk_digest_alg_by_str(optarg);
		if (!(BIT(ictx->hash_alg) & ALLOWED_HASH)) {
			apk_err(out, "hash '%s' not recognized or allowed", optarg);
			return -EINVAL;
		}
		break;
	case OPT_MKNDX_index:
		ictx->index = optarg;
		break;
	case OPT_MKNDX_output:
		ictx->output = optarg;
		break;
	case OPT_MKNDX_pkgname_spec:
		ictx->pkgname_spec = APK_BLOB_STR(optarg);
		ictx->pkgname_spec_set = 1;
		break;
	case OPT_MKNDX_rewrite_arch:
		apk_err(out, "--rewrite-arch is removed, use instead: --pkgname-spec '%s/${name}-${version}.apk'", optarg);
		return -ENOTSUP;
	default:
		return -ENOTSUP;
	}
	return 0;
}

struct field {
	apk_blob_t str;
	unsigned int ndx;
};
#define FIELD(s, n) { .str = APK_BLOB_STRLIT(s), .ndx = n }

static int cmpfield(const void *pa, const void *pb)
{
	const struct field *a = pa, *b = pb;
	return apk_blob_sort(a->str, b->str);
}

static int mkndx_parse_v2meta(struct apk_extract_ctx *ectx, struct apk_istream *is)
{
	static struct field fields[]  = {
		FIELD("arch",			ADBI_PI_ARCH),
		FIELD("builddate",		ADBI_PI_BUILD_TIME),
		FIELD("commit",			ADBI_PI_REPO_COMMIT),
		FIELD("datahash",		0),
		FIELD("depend",			ADBI_PI_DEPENDS),
		FIELD("install_if",		ADBI_PI_INSTALL_IF),
		FIELD("license",		ADBI_PI_LICENSE),
		FIELD("maintainer",		ADBI_PI_MAINTAINER),
		FIELD("origin",			ADBI_PI_ORIGIN),
		FIELD("packager",		0),
		FIELD("pkgdesc",		ADBI_PI_DESCRIPTION),
		FIELD("pkgname",		ADBI_PI_NAME),
		FIELD("pkgver", 		ADBI_PI_VERSION),
		FIELD("provider_priority",	ADBI_PI_PROVIDER_PRIORITY),
		FIELD("provides",		ADBI_PI_PROVIDES),
		FIELD("replaces",		ADBI_PI_REPLACES),
		FIELD("replaces_priority",	0),
		FIELD("size",			ADBI_PI_INSTALLED_SIZE),
		FIELD("triggers",		0),
		FIELD("url",			ADBI_PI_URL),
	};
	struct mkndx_ctx *ctx = container_of(ectx, struct mkndx_ctx, ectx);
	struct field *f, key;
	struct adb *db = &ctx->db;
	struct adb_obj deps[3];
	apk_blob_t line, k, v, token = APK_BLOB_STR("\n"), bdep;
	int r, e = 0, i = 0;

	adb_wo_alloca(&deps[0], &schema_dependency_array, db);
	adb_wo_alloca(&deps[1], &schema_dependency_array, db);
	adb_wo_alloca(&deps[2], &schema_dependency_array, db);

	while ((r = apk_istream_get_delim(is, token, &line)) == 0) {
		if (line.len < 1 || line.ptr[0] == '#') continue;
		if (!apk_blob_split(line, APK_BLOB_STR(" = "), &k, &v)) continue;
		apk_extract_v2_control(ectx, k, v);

		key.str = k;
		f = bsearch(&key, fields, ARRAY_SIZE(fields), sizeof(fields[0]), cmpfield);
		if (!f || f->ndx == 0) continue;

		if (adb_ro_val(&ctx->pkginfo, f->ndx) != ADB_NULL)
			return -APKE_ADB_PACKAGE_FORMAT;

		switch (f->ndx) {
		case ADBI_PI_DEPENDS:
			i = 0;
			goto parse_deps;
		case ADBI_PI_PROVIDES:
			i = 1;
			goto parse_deps;
		case ADBI_PI_REPLACES:
			i = 2;
		parse_deps:
			while (apk_dep_split(&v, &bdep)) {
				e = adb_wa_append_fromstring(&deps[i], bdep);
				if (ADB_IS_ERROR(e)) return -ADB_VAL_VALUE(e);
			}
			continue;
		}
		adb_wo_pkginfo(&ctx->pkginfo, f->ndx, v);
	}
	if (r != -APKE_EOF) return r;

	adb_wo_arr(&ctx->pkginfo, ADBI_PI_DEPENDS, &deps[0]);
	adb_wo_arr(&ctx->pkginfo, ADBI_PI_PROVIDES, &deps[1]);
	adb_wo_arr(&ctx->pkginfo, ADBI_PI_REPLACES, &deps[2]);

	adb_wo_free(&deps[0]);
	adb_wo_free(&deps[1]);
	adb_wo_free(&deps[2]);

	return 0;
}

static int mkndx_parse_v3meta(struct apk_extract_ctx *ectx, struct adb_obj *pkg)
{
	struct mkndx_ctx *ctx = container_of(ectx, struct mkndx_ctx, ectx);
	struct adb_obj pkginfo;

	adb_ro_obj(pkg, ADBI_PKG_PKGINFO, &pkginfo);
	adb_wo_copyobj(&ctx->pkginfo, &pkginfo);

	return 0;
}

static const struct apk_extract_ops extract_ndxinfo_ops = {
	.v2meta = mkndx_parse_v2meta,
	.v3meta = mkndx_parse_v3meta,
};

static int find_package(struct adb_obj *pkgs, apk_blob_t path, int64_t filesize, apk_blob_t pkgname_spec)
{
	char buf[NAME_MAX], split_char;
	apk_blob_t name_format, filename = path, expected_filename;
	struct adb tmpdb;
	struct adb_obj tmpl;
	int r;

	adb_w_init_tmp(&tmpdb, 200);
	adb_wo_alloca(&tmpl, &schema_pkginfo, &tmpdb);

	if (!apk_blob_rsplit(pkgname_spec, '/', NULL, &name_format)) name_format = pkgname_spec;
	if (!apk_blob_starts_with(name_format, APK_BLOB_STRLIT("${name}"))) return -APKE_PACKAGE_NAME_SPEC;
	split_char = name_format.ptr[7];

	if (apk_blob_rsplit(path, '/', NULL, &filename) && apk_blob_chr(pkgname_spec, '/')) {
		// both spec and path have path name component, so compare full paths
		expected_filename = path;
		name_format = pkgname_spec;
	} else {
		// work with the filename portion only
		expected_filename = filename;
	}

	// apk_pkg_subst_validate enforces pkgname_spec to be /${name} followed by [-._]
	// enumerate all potential names by walking the potential split points
	for (int i = 1; i < filename.len; i++) {
		if (filename.ptr[i] != split_char) continue;

		adb_wo_resetdb(&tmpl);
		adb_wo_blob(&tmpl, ADBI_PI_NAME, APK_BLOB_PTR_LEN(filename.ptr, i));
		if (filesize) adb_wo_int(&tmpl, ADBI_PI_FILE_SIZE, filesize);

		int ndx = 0;
		while ((ndx = adb_ra_find(pkgs, ndx, &tmpl)) > 0) {
			struct adb_obj pkg;
			adb_ro_obj(pkgs, ndx, &pkg);

			r = apk_blob_subst(buf, sizeof buf, name_format, adb_s_field_subst, &pkg);
			if (r < 0) continue;
			if (apk_blob_compare(expected_filename, APK_BLOB_PTR_LEN(buf, r)) == 0)
				return ndx;
		}
	}

	return -APKE_PACKAGE_NOT_FOUND;
}

static int mkndx_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct mkndx_ctx *ctx = pctx;
	struct apk_out *out = &ac->out;
	struct apk_trust *trust = apk_ctx_get_trust(ac);
	struct adb odb;
	struct adb_obj oroot, opkgs, ndx;
	struct apk_digest digest;
	struct apk_file_info fi;
	apk_blob_t lookup_spec = ctx->pkgname_spec;
	int r = -1, errors = 0, newpkgs = 0, numpkgs;
	char buf[NAME_MAX];
	time_t index_mtime = 0;

	apk_extract_init(&ctx->ectx, ac, &extract_ndxinfo_ops);

	adb_init(&odb);
	adb_w_init_alloca(&ctx->db, ADB_SCHEMA_INDEX, 8000);
	adb_wo_alloca(&ndx, &schema_index, &ctx->db);
	adb_wo_alloca(&ctx->pkgs, &schema_pkginfo_array, &ctx->db);
	adb_wo_alloca(&ctx->pkginfo, &schema_pkginfo, &ctx->db);

	if (!ctx->output) {
		apk_err(out, "Please specify --output FILE");
		goto done;
	}
	if (ctx->filter_spec_set) {
		if (!ctx->index) {
			apk_err(out, "--filter-spec requires --index");
			goto done;
		}
		lookup_spec = ctx->filter_spec;
	}
	if (ctx->index) {
		apk_fileinfo_get(AT_FDCWD, ctx->index, 0, &fi, 0);
		index_mtime = fi.mtime;

		r = adb_m_open(&odb,
			adb_decompress(apk_istream_from_file_mmap(AT_FDCWD, ctx->index), NULL),
			ADB_SCHEMA_INDEX, trust);
		if (r) {
			apk_err(out, "%s: %s", ctx->index, apk_error_str(r));
			goto done;
		}
		adb_ro_obj(adb_r_rootobj(&odb, &oroot, &schema_index), ADBI_NDX_PACKAGES, &opkgs);
	}

	apk_array_foreach_item(arg, args) {
		adb_val_t val = ADB_VAL_NULL;
		int64_t file_size = 0;
		bool use_previous = true;

		if (!ctx->filter_spec_set) {
			r = apk_fileinfo_get(AT_FDCWD, arg, 0, &fi, 0);
			if (r < 0) goto err_pkg;
			file_size = fi.size;
			use_previous = index_mtime >= fi.mtime;
		}

		if (use_previous && (r = find_package(&opkgs, APK_BLOB_STR(arg), file_size, lookup_spec)) > 0) {
			apk_dbg(out, "%s: indexed from old index", arg);
			val = adb_wa_append(&ctx->pkgs, adb_w_copy(&ctx->db, &odb, adb_ro_val(&opkgs, r)));
		}
		if (val == ADB_VAL_NULL && !ctx->filter_spec_set) {
			apk_digest_reset(&digest);
			apk_extract_reset(&ctx->ectx);
			apk_extract_generate_identity(&ctx->ectx, ctx->hash_alg, &digest);
			r = apk_extract(&ctx->ectx, apk_istream_from_file(AT_FDCWD, arg));
			if (r < 0 && r != -ECANCELED) {
				adb_wo_reset(&ctx->pkginfo);
				goto err_pkg;
			}

			adb_wo_int(&ctx->pkginfo, ADBI_PI_FILE_SIZE, file_size);
			adb_wo_blob(&ctx->pkginfo, ADBI_PI_HASHES, APK_DIGEST_BLOB(digest));

			if (ctx->pkgname_spec_set &&
			    (apk_blob_subst(buf, sizeof buf, ctx->pkgname_spec, adb_s_field_subst, &ctx->pkginfo) < 0 ||
			     strcmp(apk_last_path_segment(buf), apk_last_path_segment(arg)) != 0))
				apk_warn(out, "%s: not matching package name specification '" BLOB_FMT "'",
					arg, BLOB_PRINTF(ctx->pkgname_spec));

			apk_dbg(out, "%s: indexed new package", arg);
			val = adb_wa_append_obj(&ctx->pkgs, &ctx->pkginfo);
			newpkgs++;
		}
		if (val == ADB_VAL_NULL) continue;
		if (ADB_IS_ERROR(val)) {
			r = ADB_VAL_VALUE(val);
		err_pkg:
			apk_err(out, "%s: %s", arg, apk_error_str(r));
			errors++;
		}
	}
	if (errors) {
		apk_err(out, "%d errors, not creating index", errors);
		r = -1;
		goto done;
	}

	numpkgs = adb_ra_num(&ctx->pkgs);
	adb_wo_blob(&ndx, ADBI_NDX_DESCRIPTION, APK_BLOB_STR(ctx->description));
	if (ctx->pkgname_spec_set) adb_wo_blob(&ndx, ADBI_NDX_PKGNAME_SPEC, ctx->pkgname_spec);
	adb_wo_obj(&ndx, ADBI_NDX_PACKAGES, &ctx->pkgs);
	adb_w_rootobj(&ndx);

	r = adb_c_create(
		adb_compress(apk_ostream_to_file(AT_FDCWD, ctx->output, 0644), &ac->compspec),
		&ctx->db, trust);

	if (r == 0)
		apk_msg(out, "Index has %d packages (of which %d are new)", numpkgs, newpkgs);
	else
		apk_err(out, "Index creation failed: %s", apk_error_str(r));

done:
	adb_wo_free(&ctx->pkgs);
	adb_free(&ctx->db);
	adb_free(&odb);

#if 0
	apk_hash_foreach(&db->available.names, warn_if_no_providers, &counts);

	if (counts.unsatisfied != 0)
		apk_warn(out,
			"Total of %d unsatisfiable package names. Your repository may be broken.",
			counts.unsatisfied);
#endif

	return r;
}

static struct apk_applet apk_mkndx = {
	.name = "mkndx",
	.options_desc = mkndx_options_desc,
	.optgroup_generation = 1,
	.context_size = sizeof(struct mkndx_ctx),
	.parse = mkndx_parse_option,
	.main = mkndx_main,
};

APK_DEFINE_APPLET(apk_mkndx);
