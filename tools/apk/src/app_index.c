/* app_index.c - Alpine Package Keeper (APK)
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
#include <unistd.h>
#include <sys/stat.h>

#include "apk_applet.h"
#include "apk_database.h"
#include "apk_defines.h"
#include "apk_print.h"
#include "apk_tar.h"

#define APK_INDEXF_NO_WARNINGS	BIT(0)
#define APK_INDEXF_MERGE	BIT(1)
#define APK_INDEXF_PRUNE_ORIGIN	BIT(2)

struct counts {
	struct apk_indent indent;
	int unsatisfied;
	unsigned short header : 1;
};

struct index_ctx {
	const char *index;
	const char *output;
	const char *description;
	const char *rewrite_arch;
	time_t index_mtime;
	unsigned short index_flags;
};

#define INDEX_OPTIONS(OPT) \
	OPT(OPT_INDEX_description,	APK_OPT_ARG APK_OPT_SH("d") "description") \
	OPT(OPT_INDEX_index,		APK_OPT_ARG APK_OPT_SH("x") "index") \
	OPT(OPT_INDEX_merge,		"merge") \
	OPT(OPT_INDEX_no_warnings,	"no-warnings") \
	OPT(OPT_INDEX_output,		APK_OPT_ARG APK_OPT_SH("o") "output") \
	OPT(OPT_INDEX_prune_origin,	"prune-origin") \
	OPT(OPT_INDEX_rewrite_arch,	APK_OPT_ARG "rewrite-arch")

APK_OPTIONS(index_options_desc, INDEX_OPTIONS);

static int index_parse_option(void *ctx, struct apk_ctx *ac, int opt, const char *optarg)
{
	struct index_ctx *ictx = (struct index_ctx *) ctx;

	switch (opt) {
	case OPT_INDEX_description:
		ictx->description = optarg;
		break;
	case OPT_INDEX_index:
		ictx->index = optarg;
		break;
	case OPT_INDEX_merge:
		ictx->index_flags |= APK_INDEXF_MERGE;
		break;
	case OPT_INDEX_output:
		ictx->output = optarg;
		break;
	case OPT_INDEX_prune_origin:
		ictx->index_flags |= APK_INDEXF_PRUNE_ORIGIN;
		break;
	case OPT_INDEX_rewrite_arch:
		ictx->rewrite_arch = optarg;
		break;
	case OPT_INDEX_no_warnings:
		ictx->index_flags |= APK_INDEXF_NO_WARNINGS;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static int index_write(struct index_ctx *ictx, struct apk_database *db, struct apk_ostream *os)
{
	int count = 0;

	apk_array_foreach_item(name, apk_db_sorted_names(db)) {
		apk_array_foreach(p, apk_name_sorted_providers(name)) {
			struct apk_package *pkg = p->pkg;
			if (name != pkg->name) continue;

			switch (ictx->index_flags & (APK_INDEXF_MERGE|APK_INDEXF_PRUNE_ORIGIN)) {
			case APK_INDEXF_MERGE:
				break;
			case APK_INDEXF_MERGE|APK_INDEXF_PRUNE_ORIGIN:
				if (!pkg->marked && pkg->origin->len) {
					struct apk_name *n = apk_db_query_name(db, *pkg->origin);
					if (n && n->state_int) continue;
				}
				break;
			default:
				if (!pkg->marked) continue;
				break;
			}
			count++;
			apk_pkg_write_index_entry(pkg, os);
		}
	}
	return count;
}

static int index_read_file(struct apk_database *db, struct index_ctx *ictx)
{
	struct apk_file_info fi;

	if (ictx->index == NULL)
		return 0;
	if (apk_fileinfo_get(AT_FDCWD, ictx->index, 0, &fi, &db->atoms) < 0)
		return 0;

	ictx->index_mtime = fi.mtime;
	return apk_db_index_read_file(db, ictx->index, APK_REPO_NONE);
}

static int warn_if_no_providers(struct apk_database *db, const char *match, struct apk_name *name, void *ctx)
{
	struct counts *counts = (struct counts *) ctx;

	if (!name->is_dependency) return 0;
	if (apk_array_len(name->providers) != 0) return 0;

	if (!counts->header) {
		apk_print_indented_group(&counts->indent, 2, "WARNING: No provider for the dependencies:\n");
		counts->header = 1;
	}
	apk_print_indented(&counts->indent, APK_BLOB_STR(name->name));
	counts->unsatisfied++;
	return 0;
}

static void index_mark_package(struct apk_database *db, struct apk_package *pkg, apk_blob_t *rewrite_arch)
{
	if (rewrite_arch) pkg->arch = rewrite_arch;
	pkg->marked = 1;
	if (pkg->origin->len) apk_db_get_name(db, *pkg->origin)->state_int = 1;
}

static int index_main(void *ctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_out *out = &ac->out;
	struct apk_database *db = ac->db;
	struct counts counts = { .unsatisfied=0 };
	struct apk_ostream *os, *counter;
	struct apk_file_info fi;
	int total, r, newpkgs = 0, errors = 0;
	struct index_ctx *ictx = (struct index_ctx *) ctx;
	struct apk_package *pkg;
	apk_blob_t *rewrite_arch = NULL;

	if (isatty(STDOUT_FILENO) && ictx->output == NULL &&
	    !(db->ctx->force & APK_FORCE_BINARY_STDOUT)) {
		apk_err(out,
			"Will not write binary index to console. "
			"Use --force-binary-stdout to override.");
		return -1;
	}

	if ((r = index_read_file(db, ictx)) < 0) {
		apk_err(out, "%s: %s", ictx->index, apk_error_str(r));
		return r;
	}

	if (ictx->rewrite_arch)
		rewrite_arch = apk_atomize_dup(&db->atoms, APK_BLOB_STR(ictx->rewrite_arch));

	apk_array_foreach_item(arg, args) {
		if (apk_fileinfo_get(AT_FDCWD, arg, 0, &fi, &db->atoms) < 0) {
			apk_warn(out, "File '%s' is unaccessible", arg);
			continue;
		}

		if (ictx->index && ictx->index_mtime >= fi.mtime) {
			apk_blob_t fname = APK_BLOB_STR(arg);
			apk_blob_rsplit(fname, '/', NULL, &fname);
			pkg = apk_db_get_pkg_by_name(db, fname, fi.size, APK_BLOB_NULL);
			if (pkg) {
				apk_dbg(out, "%s: indexed from old index", arg);
				index_mark_package(db, pkg, rewrite_arch);
				continue;
			}
		}

		r = apk_pkg_read(db, arg, &pkg, false);
		if (r < 0) {
			apk_err(out, "%s: %s", arg, apk_error_str(r));
			errors++;
		} else {
			apk_dbg(out, "%s: indexed new package", arg);
			index_mark_package(db, pkg, rewrite_arch);
			newpkgs++;
		}
	}
	if (errors)
		return -1;

	if (ictx->output != NULL)
		os = apk_ostream_to_file(AT_FDCWD, ictx->output, 0644);
	else
		os = apk_ostream_to_fd(STDOUT_FILENO);
	if (IS_ERR(os)) return PTR_ERR(os);

	time_t mtime = apk_get_build_time(time(NULL));
	memset(&fi, 0, sizeof(fi));
	fi.mode = 0644 | S_IFREG;
	fi.name = "APKINDEX";
	fi.mtime = mtime;
	counter = apk_ostream_counter(&fi.size);
	index_write(ictx, db, counter);
	apk_ostream_close(counter);

	os = apk_ostream_gzip(os);
	if (ictx->description) {
		struct apk_file_info fi_desc;
		memset(&fi_desc, 0, sizeof(fi));
		fi_desc.mode = 0644 | S_IFREG;
		fi_desc.name = "DESCRIPTION";
		fi_desc.size = strlen(ictx->description);
		fi_desc.mtime = mtime;
		apk_tar_write_entry(os, &fi_desc, ictx->description);
	}

	apk_tar_write_entry(os, &fi, NULL);
	total = index_write(ictx, db, os);
	apk_tar_write_padding(os, fi.size);
	apk_tar_write_entry(os, NULL, NULL);

	r = apk_ostream_close(os);
	if (r < 0) {
		apk_err(out, "Index generation failed: %s", apk_error_str(r));
		return r;
	}

	if (!(ictx->index_flags & APK_INDEXF_NO_WARNINGS)) {
		apk_print_indented_init(&counts.indent, out, 1);
		apk_db_foreach_sorted_name(db, NULL, warn_if_no_providers, &counts);
		apk_print_indented_end(&counts.indent);
	}

	if (counts.unsatisfied != 0)
		apk_warn(out,
			"Total of %d unsatisfiable package names. Your repository may be broken.",
			counts.unsatisfied);
	if (ictx->output != NULL)
		apk_msg(out, "Index has %d packages (of which %d are new)",
			total, newpkgs);
	return 0;
}

static struct apk_applet apk_index = {
	.name = "index",
	.options_desc = index_options_desc,
	.open_flags = APK_OPENF_READ | APK_OPENF_NO_STATE | APK_OPENF_NO_REPOS,
	.context_size = sizeof(struct index_ctx),
	.parse = index_parse_option,
	.main = index_main,
};

APK_DEFINE_APPLET(apk_index);
