/* apk_context.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2020 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include "apk_blob.h"
#include "apk_print.h"
#include "apk_trust.h"
#include "apk_io.h"
#include "apk_crypto.h"
#include "apk_balloc.h"
#include "apk_query.h"
#include "adb.h"

#define APK_SIMULATE			BIT(0)
#define APK_CLEAN_PROTECTED		BIT(1)
#define APK_RECURSIVE			BIT(2)
#define APK_ALLOW_UNTRUSTED		BIT(3)
#define APK_PURGE			BIT(4)
#define APK_NO_NETWORK			BIT(6)
#define APK_OVERLAY_FROM_STDIN		BIT(7)
#define APK_NO_SCRIPTS			BIT(8)
#define APK_NO_CACHE			BIT(9)
#define APK_NO_COMMIT_HOOKS		BIT(10)
#define APK_NO_CHROOT			BIT(11)
#define APK_NO_LOGFILE			BIT(12)
#define APK_PRESERVE_ENV		BIT(13)

#define APK_FORCE_OVERWRITE		BIT(0)
#define APK_FORCE_OLD_APK		BIT(1)
#define APK_FORCE_BROKEN_WORLD		BIT(2)
#define APK_FORCE_REFRESH		BIT(3)
#define APK_FORCE_NON_REPOSITORY	BIT(4)
#define APK_FORCE_BINARY_STDOUT		BIT(5)
#define APK_FORCE_MISSING_REPOSITORIES	BIT(6)

#define APK_OPENF_READ			0x0001
#define APK_OPENF_WRITE			0x0002
#define APK_OPENF_CREATE		0x0004
#define APK_OPENF_NO_INSTALLED		0x0010
#define APK_OPENF_NO_SCRIPTS		0x0020
#define APK_OPENF_NO_WORLD		0x0040
#define APK_OPENF_NO_SYS_REPOS		0x0100
#define APK_OPENF_NO_INSTALLED_REPO	0x0200
#define APK_OPENF_CACHE_WRITE		0x0400
#define APK_OPENF_NO_AUTOUPDATE		0x0800
#define APK_OPENF_NO_CMDLINE_REPOS	0x1000
#define APK_OPENF_USERMODE		0x2000
#define APK_OPENF_ALLOW_ARCH		0x4000

#define APK_OPENF_NO_REPOS	(APK_OPENF_NO_SYS_REPOS |	\
				 APK_OPENF_NO_CMDLINE_REPOS |	\
				 APK_OPENF_NO_INSTALLED_REPO)
#define APK_OPENF_NO_STATE	(APK_OPENF_NO_INSTALLED |	\
				 APK_OPENF_NO_SCRIPTS |		\
				 APK_OPENF_NO_WORLD)

struct apk_database;

struct apk_ctx {
	struct apk_balloc ba;
	unsigned int flags, force, open_flags;
	unsigned int lock_wait, cache_max_age;
	struct apk_out out;
	struct adb_compression_spec compspec;
	const char *root;
	const char *keys_dir;
	const char *cache_dir;
	const char *repositories_file;
	const char *uvol;
	const char *apknew_suffix;
	apk_blob_t default_pkgname_spec;
	apk_blob_t default_reponame_spec;
	apk_blob_t default_cachename_spec;
	struct apk_string_array *repository_list;
	struct apk_string_array *repository_config_list;
	struct apk_string_array *arch_list;
	struct apk_string_array *script_environment;
	struct apk_string_array *preupgrade_deps;
	struct apk_istream *protected_paths;

	struct apk_digest_ctx dctx;
	struct apk_trust trust;
	struct apk_id_cache id_cache;
	struct apk_database *db;
	struct apk_query_spec query;
	int root_fd, dest_fd;
	unsigned int on_tty : 1;
	unsigned int root_set : 1;
	unsigned int cache_dir_set : 1;
	unsigned int cache_packages : 1;
	unsigned int cache_predownload : 1;
	unsigned int keys_loaded : 1;
	unsigned int legacy_info : 1;
	unsigned int interactive : 2;
	unsigned int root_tmpfs : 2;
	unsigned int sync : 2;
	unsigned int pretty_print : 2;
};

void apk_ctx_init(struct apk_ctx *ac);
void apk_ctx_free(struct apk_ctx *ac);
int apk_ctx_prepare(struct apk_ctx *ac);

struct apk_trust *apk_ctx_get_trust(struct apk_ctx *ac);
struct apk_id_cache *apk_ctx_get_id_cache(struct apk_ctx *ac);

static inline int apk_ctx_fd_root(struct apk_ctx *ac) { return ac->root_fd; }
static inline int apk_ctx_fd_dest(struct apk_ctx *ac) { return ac->dest_fd; }
static inline time_t apk_ctx_since(struct apk_ctx *ac, time_t since) {
	return (ac->force & APK_FORCE_REFRESH) ? APK_ISTREAM_FORCE_REFRESH : since;
}
static inline const char *apk_ctx_get_uvol(struct apk_ctx *ac) { return ac->uvol; }
