/* context.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2020 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <errno.h>
#include <unistd.h>
#include <sys/stat.h>
#include "apk_context.h"
#include "apk_fs.h"

void apk_ctx_init(struct apk_ctx *ac)
{
	memset(ac, 0, sizeof *ac);
	apk_balloc_init(&ac->ba, 64*1024);
	apk_string_array_init(&ac->repository_list);
	apk_string_array_init(&ac->repository_config_list);
	apk_string_array_init(&ac->arch_list);
	apk_string_array_init(&ac->script_environment);
	apk_string_array_init(&ac->preupgrade_deps);
	apk_trust_init(&ac->trust);
	apk_out_reset(&ac->out);
	ac->out.out = stdout;
	ac->out.err = stderr;
	ac->out.verbosity = 1;
	ac->cache_max_age = 4*60*60; /* 4 hours default */
	apk_id_cache_init(&ac->id_cache, -1);
	ac->root_fd = -1;
	ac->legacy_info = 1;
	ac->root_tmpfs = APK_AUTO;
	ac->sync = APK_AUTO;
	ac->apknew_suffix = ".apk-new";
	ac->default_pkgname_spec = APK_BLOB_STRLIT("${name}-${version}.apk");
	ac->default_reponame_spec = APK_BLOB_STRLIT("${arch}/${name}-${version}.apk");;
	ac->default_cachename_spec = APK_BLOB_STRLIT("${name}-${version}.${hash:8}.apk");
	apk_digest_ctx_init(&ac->dctx, APK_DIGEST_SHA256);
}

void apk_ctx_free(struct apk_ctx *ac)
{
	if (ac->protected_paths) apk_istream_close(ac->protected_paths);
	apk_digest_ctx_free(&ac->dctx);
	apk_id_cache_free(&ac->id_cache);
	apk_trust_free(&ac->trust);
	apk_string_array_free(&ac->preupgrade_deps);
	apk_string_array_free(&ac->repository_config_list);
	apk_string_array_free(&ac->repository_list);
	apk_string_array_free(&ac->arch_list);
	apk_string_array_free(&ac->script_environment);
	if (ac->root_fd >= 0) close(ac->root_fd);
	if (ac->out.log) fclose(ac->out.log);
	apk_balloc_destroy(&ac->ba);
}

int apk_ctx_prepare(struct apk_ctx *ac)
{
	apk_out_configure_progress(&ac->out, ac->on_tty);
	if (ac->interactive == APK_AUTO) ac->interactive = ac->on_tty;
	if (ac->pretty_print == APK_AUTO) ac->pretty_print = ac->on_tty;
	if (ac->flags & APK_SIMULATE &&
	    ac->open_flags & (APK_OPENF_CREATE | APK_OPENF_WRITE)) {
		ac->open_flags &= ~(APK_OPENF_CREATE | APK_OPENF_WRITE);
		ac->open_flags |= APK_OPENF_READ;
	}
	if (ac->flags & APK_ALLOW_UNTRUSTED) ac->trust.allow_untrusted = 1;
	if (!ac->cache_dir) ac->cache_dir = "etc/apk/cache";
	else ac->cache_dir_set = 1;
	if (!ac->root) ac->root = "/";
	if (ac->cache_predownload) ac->cache_packages = 1;

	if (!strcmp(ac->root, "/")) {
		// No chroot needed if using system root
		ac->flags |= APK_NO_CHROOT;

		// Check uvol availability
		if (!ac->uvol) ac->uvol = "/usr/sbin/uvol";
	} else {
		ac->root_set = 1;
		if (!ac->uvol) ac->uvol = ERR_PTR(-APKE_UVOL_ROOT);
	}
	if (!IS_ERR(ac->uvol) && (ac->uvol[0] != '/' || access(ac->uvol, X_OK) != 0))
		ac->uvol = ERR_PTR(-APKE_UVOL_NOT_AVAILABLE);

	ac->root_fd = openat(AT_FDCWD, ac->root, O_DIRECTORY | O_RDONLY | O_CLOEXEC);
	if (ac->root_fd < 0 && (ac->open_flags & APK_OPENF_CREATE)) {
		mkdirat(AT_FDCWD, ac->root, 0755);
		ac->root_fd = openat(AT_FDCWD, ac->root, O_DIRECTORY | O_RDONLY | O_CLOEXEC);
	}
	if (ac->root_fd < 0) {
		apk_err(&ac->out, "Unable to open root: %s", apk_error_str(errno));
		return -errno;
	}
	ac->dest_fd = ac->root_fd;

	if (ac->open_flags & APK_OPENF_CREATE) {
		uid_t uid = getuid();
		if (ac->open_flags & APK_OPENF_USERMODE) {
			if (uid == 0) {
				apk_err(&ac->out, "--usermode not allowed as root");
				return -EINVAL;
			}
		} else {
			if (uid != 0) {
				apk_err(&ac->out, "Use --usermode to allow creating database as non-root");
				return -EINVAL;
			}
		}
	}

	if ((ac->open_flags & APK_OPENF_WRITE) && !(ac->flags & APK_NO_LOGFILE)) {
		const char *log_path = "var/log/apk.log";
		const int lflags = O_WRONLY | O_APPEND | O_CREAT | O_CLOEXEC;
		int fd = openat(ac->root_fd, log_path, lflags, 0644);
		if (fd < 0) {
			apk_make_dirs(ac->root_fd, "var/log", 0755, 0755);
			fd = openat(ac->root_fd, log_path, lflags, 0644);
		}
		if (fd < 0) {
			apk_err(&ac->out, "Unable to open log: %s", apk_error_str(errno));
			return -errno;
		}
		ac->out.log = fdopen(fd, "a");
	}

	if (ac->flags & APK_PRESERVE_ENV) {
		for (int i = 0; environ[i]; i++)
			if (strncmp(environ[i], "APK_", 4) != 0)
				apk_string_array_add(&ac->script_environment, environ[i]);
	} else {
		apk_string_array_add(&ac->script_environment, "PATH=/usr/sbin:/usr/bin:/sbin:/bin");
	}

	return 0;
}

static int __apk_ctx_load_pubkey(void *pctx, int dirfd, const char *path, const char *filename)
{
	struct apk_trust *trust = pctx;
	struct apk_trust_key *key = apk_trust_load_key(dirfd, filename, 0);

	if (!IS_ERR(key))
		list_add_tail(&key->key_node, &trust->trusted_key_list);

	return 0;
}

struct apk_trust *apk_ctx_get_trust(struct apk_ctx *ac)
{
	if (!ac->keys_loaded) {
		if (!ac->keys_dir) {
			apk_dir_foreach_config_file(ac->root_fd,
				__apk_ctx_load_pubkey, &ac->trust,
				apk_filename_is_hidden,
				"etc/apk/keys",
				"lib/apk/keys",
				NULL);
		} else {
			apk_dir_foreach_file(ac->root_fd, ac->keys_dir,
				__apk_ctx_load_pubkey, &ac->trust,
				apk_filename_is_hidden);
		}
		ac->keys_loaded = 1;
	}
	return &ac->trust;
}

struct apk_id_cache *apk_ctx_get_id_cache(struct apk_ctx *ac)
{
	if (ac->id_cache.root_fd < 0)
		apk_id_cache_reset_rootfd(&ac->id_cache, apk_ctx_fd_root(ac));
	return &ac->id_cache;
}
