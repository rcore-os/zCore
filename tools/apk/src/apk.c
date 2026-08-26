/* apk.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <stdio.h>
#include <fcntl.h>
#include <ctype.h>
#include <errno.h>
#include <signal.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/stat.h>

#include "apk_defines.h"
#include "apk_arch.h"
#include "apk_database.h"
#include "apk_applet.h"
#include "apk_blob.h"
#include "apk_print.h"
#include "apk_io.h"
#include "apk_fs.h"

static struct apk_ctx ctx;
static struct apk_database db;
static struct apk_applet *applet;
static void *applet_ctx;
char **apk_argv;
int apk_argc;

static void version(struct apk_out *out, const char *prefix)
{
	apk_out_fmt(out, prefix, "apk-tools " APK_VERSION ", compiled for " APK_DEFAULT_ARCH ".");
}

#define GLOBAL_OPTIONS(OPT) \
	OPT(OPT_GLOBAL_allow_untrusted,		"allow-untrusted") \
	OPT(OPT_GLOBAL_arch,			APK_OPT_ARG "arch") \
	OPT(OPT_GLOBAL_cache,			APK_OPT_BOOL "cache") \
	OPT(OPT_GLOBAL_cache_dir,		APK_OPT_ARG "cache-dir") \
	OPT(OPT_GLOBAL_cache_max_age,		APK_OPT_ARG "cache-max-age") \
	OPT(OPT_GLOBAL_cache_packages,		APK_OPT_BOOL "cache-packages") \
	OPT(OPT_GLOBAL_cache_predownload,	APK_OPT_BOOL "cache-predownload") \
	OPT(OPT_GLOBAL_check_certificate,	APK_OPT_BOOL "check-certificate") \
	OPT(OPT_GLOBAL_force,			APK_OPT_SH("f") "force") \
	OPT(OPT_GLOBAL_force_binary_stdout,	"force-binary-stdout") \
	OPT(OPT_GLOBAL_force_broken_world,	"force-broken-world") \
	OPT(OPT_GLOBAL_force_missing_repositories, "force-missing-repositories") \
	OPT(OPT_GLOBAL_force_no_chroot,		"force-no-chroot") \
	OPT(OPT_GLOBAL_force_non_repository,	"force-non-repository") \
	OPT(OPT_GLOBAL_force_old_apk,		"force-old-apk") \
	OPT(OPT_GLOBAL_force_overwrite,		"force-overwrite") \
	OPT(OPT_GLOBAL_force_refresh,		"force-refresh") \
	OPT(OPT_GLOBAL_help,			APK_OPT_SH("h") "help") \
	OPT(OPT_GLOBAL_interactive,		APK_OPT_AUTO APK_OPT_SH("i") "interactive") \
	OPT(OPT_GLOBAL_keys_dir,		APK_OPT_ARG "keys-dir") \
	OPT(OPT_GLOBAL_legacy_info,		APK_OPT_BOOL "legacy-info") \
	OPT(OPT_GLOBAL_logfile,			APK_OPT_BOOL "logfile") \
	OPT(OPT_GLOBAL_network,			APK_OPT_BOOL "network") \
	OPT(OPT_GLOBAL_preserve_env,		APK_OPT_BOOL "preserve-env") \
	OPT(OPT_GLOBAL_pretty_print,		APK_OPT_AUTO "pretty-print") \
	OPT(OPT_GLOBAL_preupgrade_depends,	APK_OPT_ARG "preupgrade-depends") \
	OPT(OPT_GLOBAL_print_arch,		"print-arch") \
	OPT(OPT_GLOBAL_progress,		APK_OPT_AUTO "progress") \
	OPT(OPT_GLOBAL_progress_fd,		APK_OPT_ARG "progress-fd") \
	OPT(OPT_GLOBAL_purge,			APK_OPT_BOOL "purge") \
	OPT(OPT_GLOBAL_quiet,			APK_OPT_SH("q") "quiet") \
	OPT(OPT_GLOBAL_repositories_file,	APK_OPT_ARG "repositories-file") \
	OPT(OPT_GLOBAL_repository,		APK_OPT_ARG APK_OPT_SH("X") "repository") \
	OPT(OPT_GLOBAL_repository_config,	APK_OPT_ARG "repository-config") \
	OPT(OPT_GLOBAL_root,			APK_OPT_ARG APK_OPT_SH("p") "root") \
	OPT(OPT_GLOBAL_root_tmpfs,		APK_OPT_AUTO "root-tmpfs") \
	OPT(OPT_GLOBAL_sync,			APK_OPT_AUTO "sync") \
	OPT(OPT_GLOBAL_timeout,			APK_OPT_ARG "timeout") \
	OPT(OPT_GLOBAL_update_cache,		APK_OPT_SH("U") "update-cache") \
	OPT(OPT_GLOBAL_uvol_manager,		APK_OPT_ARG "uvol-manager") \
	OPT(OPT_GLOBAL_verbose,			APK_OPT_SH("v") "verbose") \
	OPT(OPT_GLOBAL_version,			APK_OPT_SH("V") "version") \
	OPT(OPT_GLOBAL_wait,			APK_OPT_ARG "wait") \

APK_OPTIONS(optgroup_global_desc, GLOBAL_OPTIONS);

static int optgroup_global_parse(struct apk_ctx *ac, int opt, const char *optarg)
{
	struct apk_out *out = &ac->out;
	switch (opt) {
	case OPT_GLOBAL_allow_untrusted:
		ac->flags |= APK_ALLOW_UNTRUSTED;
		break;
	case OPT_GLOBAL_arch:
		apk_string_array_add(&ac->arch_list, (char*) optarg);
		break;
	case OPT_GLOBAL_cache:
		apk_opt_set_flag_invert(optarg, APK_NO_CACHE, &ac->flags);
		break;
	case OPT_GLOBAL_cache_dir:
		ac->cache_dir = optarg;
		break;
	case OPT_GLOBAL_cache_max_age:
		ac->cache_max_age = atoi(optarg) * 60;
		break;
	case OPT_GLOBAL_cache_packages:
		ac->cache_packages = APK_OPTARG_VAL(optarg);
		break;
	case OPT_GLOBAL_cache_predownload:
		ac->cache_predownload = APK_OPTARG_VAL(optarg);
		break;
	case OPT_GLOBAL_check_certificate:
		apk_io_url_check_certificate(APK_OPTARG_VAL(optarg));
		break;
	case OPT_GLOBAL_force:
		ac->force |= APK_FORCE_OVERWRITE | APK_FORCE_OLD_APK
			| APK_FORCE_NON_REPOSITORY | APK_FORCE_BINARY_STDOUT;
		break;
	case OPT_GLOBAL_force_overwrite:
		ac->force |= APK_FORCE_OVERWRITE;
		break;
	case OPT_GLOBAL_force_old_apk:
		ac->force |= APK_FORCE_OLD_APK;
		break;
	case OPT_GLOBAL_force_broken_world:
		ac->force |= APK_FORCE_BROKEN_WORLD;
		break;
	case OPT_GLOBAL_force_refresh:
		ac->force |= APK_FORCE_REFRESH;
		break;
	case OPT_GLOBAL_force_no_chroot:
		ac->flags |= APK_NO_CHROOT;
		break;
	case OPT_GLOBAL_force_non_repository:
		ac->force |= APK_FORCE_NON_REPOSITORY;
		break;
	case OPT_GLOBAL_force_binary_stdout:
		ac->force |= APK_FORCE_BINARY_STDOUT;
		break;
	case OPT_GLOBAL_force_missing_repositories:
		ac->force |= APK_FORCE_MISSING_REPOSITORIES;
		break;
	case OPT_GLOBAL_help:
		return -ENOTSUP;
	case OPT_GLOBAL_interactive:
		ac->interactive = APK_OPTARG_VAL(optarg);
		break;
	case OPT_GLOBAL_keys_dir:
		ac->keys_dir = optarg;
		break;
	case OPT_GLOBAL_legacy_info:
		ac->legacy_info = APK_OPTARG_VAL(optarg);
		break;
	case OPT_GLOBAL_logfile:
		apk_opt_set_flag_invert(optarg, APK_NO_LOGFILE, &ac->flags);
		break;
	case OPT_GLOBAL_network:
		apk_opt_set_flag_invert(optarg, APK_NO_NETWORK, &ac->flags);
		break;
	case OPT_GLOBAL_preserve_env:
		apk_opt_set_flag(optarg, APK_PRESERVE_ENV, &ac->flags);
		break;
	case OPT_GLOBAL_pretty_print:
		ac->pretty_print = APK_OPTARG_VAL(optarg);
		break;
	case OPT_GLOBAL_preupgrade_depends:
		apk_string_array_add(&ac->preupgrade_deps, (char*) optarg);
		break;
	case OPT_GLOBAL_print_arch:
		puts(APK_DEFAULT_ARCH);
		return -ESHUTDOWN;
	case OPT_GLOBAL_progress:
		ac->out.progress = APK_OPTARG_VAL(optarg);
		break;
	case OPT_GLOBAL_progress_fd:
		ac->out.progress_fd = atoi(optarg);
		break;
	case OPT_GLOBAL_purge:
		apk_opt_set_flag(optarg, APK_PURGE, &ac->flags);
		break;
	case OPT_GLOBAL_quiet:
		if (ac->out.verbosity) ac->out.verbosity--;
		break;
	case OPT_GLOBAL_repositories_file:
		ac->repositories_file = optarg;
		break;
	case OPT_GLOBAL_repository:
		apk_string_array_add(&ac->repository_list, (char*) optarg);
		break;
	case OPT_GLOBAL_repository_config:
		apk_string_array_add(&ac->repository_config_list, (char*) optarg);
		break;
	case OPT_GLOBAL_root:
		ac->root = optarg;
		break;
	case OPT_GLOBAL_root_tmpfs:
		ac->root_tmpfs = APK_OPTARG_VAL(optarg);
		break;
	case OPT_GLOBAL_sync:
		ac->sync = APK_OPTARG_VAL(optarg);
		break;
	case OPT_GLOBAL_timeout:
		apk_io_url_set_timeout(atoi(optarg));
		break;
	case OPT_GLOBAL_update_cache:
		ac->cache_max_age = 0;
		break;
	case OPT_GLOBAL_uvol_manager:
		ac->uvol = optarg;
		break;
	case OPT_GLOBAL_verbose:
		ac->out.verbosity++;
		break;
	case OPT_GLOBAL_version:
		version(out, NULL);
		return -ESHUTDOWN;
	case OPT_GLOBAL_wait:
		ac->lock_wait = atoi(optarg);
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

#define COMMIT_OPTIONS(OPT) \
	OPT(OPT_COMMIT_clean_protected,		APK_OPT_BOOL "clean-protected") \
	OPT(OPT_COMMIT_commit_hooks,		APK_OPT_BOOL "commit-hooks") \
	OPT(OPT_COMMIT_initramfs_diskless_boot,	"initramfs-diskless-boot") \
	OPT(OPT_COMMIT_overlay_from_stdin,	"overlay-from-stdin") \
	OPT(OPT_COMMIT_scripts,			APK_OPT_BOOL "scripts") \
	OPT(OPT_COMMIT_simulate,		APK_OPT_BOOL APK_OPT_SH("s") "simulate")

APK_OPTIONS(optgroup_commit_desc, COMMIT_OPTIONS);

static int optgroup_commit_parse(struct apk_ctx *ac, int opt, const char *optarg)
{
	switch (opt) {
	case OPT_COMMIT_clean_protected:
		apk_opt_set_flag(optarg, APK_CLEAN_PROTECTED, &ac->flags);
		break;
	case OPT_COMMIT_commit_hooks:
		apk_opt_set_flag_invert(optarg, APK_NO_COMMIT_HOOKS, &ac->flags);
		break;
	case OPT_COMMIT_initramfs_diskless_boot:
		ac->open_flags |= APK_OPENF_CREATE;
		ac->flags |= APK_NO_COMMIT_HOOKS;
		ac->force |= APK_FORCE_OVERWRITE | APK_FORCE_OLD_APK
			|  APK_FORCE_BROKEN_WORLD | APK_FORCE_NON_REPOSITORY;
		break;
	case OPT_COMMIT_overlay_from_stdin:
		ac->flags |= APK_OVERLAY_FROM_STDIN;
		break;
	case OPT_COMMIT_scripts:
		apk_opt_set_flag_invert(optarg, APK_NO_SCRIPTS, &ac->flags);
		break;
	case OPT_COMMIT_simulate:
		apk_opt_set_flag(optarg, APK_SIMULATE, &ac->flags);
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

#define GENERATION_OPTIONS(OPT) \
	OPT(OPT_GENERATION_compression,	APK_OPT_ARG APK_OPT_SH("c") "compression") \
	OPT(OPT_GENERATION_sign_key,	APK_OPT_ARG "sign-key")

APK_OPTIONS(optgroup_generation_desc, GENERATION_OPTIONS);

int optgroup_generation_parse(struct apk_ctx *ac, int optch, const char *optarg)
{
	struct apk_trust *trust = &ac->trust;
	struct apk_out *out = &ac->out;
	struct apk_trust_key *key;

	switch (optch) {
	case OPT_GENERATION_compression:
		if (adb_parse_compression(optarg, &ac->compspec) != 0)
			return -EINVAL;
		break;
	case OPT_GENERATION_sign_key:
		key = apk_trust_load_key(AT_FDCWD, optarg, 1);
		if (IS_ERR(key)) {
			apk_err(out, "Failed to load signing key: %s: %s",
				optarg, apk_error_str(PTR_ERR(key)));
			return PTR_ERR(key);
		}
		list_add_tail(&key->key_node, &trust->private_key_list);
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static int usage(struct apk_out *out)
{
	version(out, NULL);
	apk_applet_help(applet, out);
	return 1;
}

struct apk_opt_match {
	apk_blob_t key;
	const char *value;
	int (*func)(struct apk_ctx *, int, const char *);
	unsigned int cnt;
	unsigned int optid;
	const char *optarg;
	char short_opt;
	bool value_explicit, value_used;
};

enum {
	OPT_MATCH_PARTIAL = 1,
	OPT_MATCH_EXACT,
	OPT_MATCH_INVALID,
	OPT_MATCH_AMBIGUOUS,
	OPT_MATCH_ARGUMENT_EXPECTED,
	OPT_MATCH_ARGUMENT_UNEXPECTED,
	OPT_MATCH_NON_OPTION
};

static int opt_parse_yesnoauto(const char *arg, bool auto_arg)
{
	if (strcmp(arg, "yes") == 0) return APK_YES;
	if (strcmp(arg, "no") == 0) return APK_NO;
	if (auto_arg && strcmp(arg, "auto") == 0) return APK_AUTO;
	return -EINVAL;
}

static int opt_parse_desc(struct apk_opt_match *m, const char *desc, int (*func)(struct apk_ctx *, int, const char *))
{
	bool no_prefix = apk_blob_starts_with(m->key, APK_BLOB_STRLIT("no-"));
	int id = 0;
	for (const char *d = desc; *d; d += strlen(d) + 1, id++) {
		const void *arg = m->value;
		bool value_used = false, bool_arg = false, auto_arg = false;
		while ((unsigned char)*d >= 0xa0) {
			switch ((unsigned char)*d++) {
			case 0xa0:
				if (*d++ != m->short_opt) break;
				if (m->cnt) return OPT_MATCH_AMBIGUOUS;
				m->cnt++;
				m->func = func;
				m->optid = id;
				if (bool_arg) {
					m->optarg = APK_OPTARG(APK_YES);
					m->value_used = false;
				} else {
					m->optarg = arg;
					m->value_used = value_used;
				}
				return OPT_MATCH_EXACT;
			case 0xaa:
				auto_arg = bool_arg = true;
				break;
			case 0xab:
				bool_arg = true;
				break;
			case 0xaf:
				value_used = true;
				break;
			}
		}
		if (m->short_opt) continue;
		size_t dlen = 0;
		if (strncmp(m->key.ptr, d, m->key.len) == 0)
			dlen = strnlen(d, m->key.len+1);
		else if (bool_arg && no_prefix && strncmp(m->key.ptr+3, d, m->key.len-3) == 0)
			dlen = strnlen(d, m->key.len-3+1) + 3;
		if (dlen >= m->key.len) {
			m->cnt++;
			m->func = func;
			m->optid = id;
			if (bool_arg) {
				if (no_prefix) {
					m->optarg = APK_OPTARG(APK_NO);
					m->value_used = false;
				} else if (!m->value_explicit) {
					m->optarg = APK_OPTARG(APK_YES);
					m->value_used = false;
				} else {
					int r = opt_parse_yesnoauto(m->value, auto_arg);
					if (r < 0) return r;
					m->optarg = APK_OPTARG(r);
					m->value_used = true;
				}
			} else {
				m->optarg = value_used ? arg : NULL;
				m->value_used = value_used;
			}
			if (dlen == m->key.len) return OPT_MATCH_EXACT;
		}
	}
	return 0;
}

static int optgroup_applet_parse(struct apk_ctx *ac, int opt, const char *val)
{
	return applet->parse(applet_ctx, ac, opt, val);
}

static int opt_match(struct apk_opt_match *m)
{
	int r;
	if ((r = opt_parse_desc(m, optgroup_global_desc, optgroup_global_parse)) != 0) goto done;
	if (applet) {
		if (applet->options_desc && (r=opt_parse_desc(m, applet->options_desc, optgroup_applet_parse)) != 0) goto done;
		if (applet->optgroup_commit && (r=opt_parse_desc(m, optgroup_commit_desc, optgroup_commit_parse)) != 0) goto done;
		if (applet->optgroup_query && (r=opt_parse_desc(m, optgroup_query_desc, apk_query_parse_option)) != 0) goto done;
		if (applet->optgroup_generation && (r=opt_parse_desc(m, optgroup_generation_desc, optgroup_generation_parse)) != 0) goto done;
	}
	if (m->cnt != 1) return (m->cnt > 1) ? OPT_MATCH_AMBIGUOUS : OPT_MATCH_INVALID;
	r = OPT_MATCH_PARTIAL;
done:
	if (r != OPT_MATCH_PARTIAL && r != OPT_MATCH_EXACT) return r;
	if (m->value_used && !m->value) r = OPT_MATCH_ARGUMENT_EXPECTED;
	if (!m->value_used && m->value_explicit) r = OPT_MATCH_ARGUMENT_UNEXPECTED;
	return r;
}

static void opt_print_error(int r, const char *fmtprefix, const char *prefix, struct apk_opt_match *m, struct apk_out *out)
{
	switch (r) {
	case OPT_MATCH_PARTIAL:
	case OPT_MATCH_INVALID:
		apk_out_fmt(out, fmtprefix, "%s: unrecognized option '" BLOB_FMT "'",
			prefix, BLOB_PRINTF(m->key));
		break;
	case OPT_MATCH_AMBIGUOUS:
		apk_out_fmt(out, fmtprefix, "%s: ambiguous option '" BLOB_FMT "'",
			prefix, BLOB_PRINTF(m->key));
		break;
	case OPT_MATCH_ARGUMENT_UNEXPECTED:
		apk_out_fmt(out, fmtprefix, "%s: option '" BLOB_FMT "' does not expect argument (got '%s')",
			prefix, BLOB_PRINTF(m->key), m->value);
		break;
	case OPT_MATCH_ARGUMENT_EXPECTED:
		apk_out_fmt(out, fmtprefix, "%s: option '" BLOB_FMT "' expects an argument",
			prefix, BLOB_PRINTF(m->key));
		break;
	case -EINVAL:
		apk_out_fmt(out, fmtprefix, "%s: invalid argument for option '" BLOB_FMT "': '%s'",
			prefix, BLOB_PRINTF(m->key), m->value);
		break;
	default:
		apk_out_fmt(out, fmtprefix, "%s: setting option '" BLOB_FMT "' failed",
			prefix, BLOB_PRINTF(m->key));
		break;
	}
}

struct opt_parse_state {
	char **argv;
	int argc;
	bool execute;
	bool end_of_options;
};

static struct opt_parse_state opt_parse_init(int argc, char **argv, bool execute) {
	return (struct opt_parse_state) { .argc = argc - 1, .argv = argv + 1, .execute = execute };
}
static bool opt_parse_ok(struct opt_parse_state *st) { return st->argc > 0; }
static void opt_parse_next(struct opt_parse_state *st) { st->argv++, st->argc--; }
static char *opt_parse_arg(struct opt_parse_state *st) { return st->argv[0]; }
static char *opt_parse_next_arg(struct opt_parse_state *st) { return (st->argc > 0) ? st->argv[1] : 0; }

static int opt_parse_argv(struct opt_parse_state *st, struct apk_opt_match *m, struct apk_ctx *ac)
{
	const char *arg = opt_parse_arg(st), *next_arg = opt_parse_next_arg(st);
	if (st->end_of_options) return OPT_MATCH_NON_OPTION;
	if (arg[0] != '-' || arg[1] == 0) return OPT_MATCH_NON_OPTION;
	if (arg[1] == '-') {
		if (arg[2] == 0) {
			st->end_of_options = true;
			return 0;
		}
		apk_blob_t val;
		*m = (struct apk_opt_match) {
			.key = APK_BLOB_STR(arg+2),
			.value = next_arg,
		};
		if (apk_blob_split(m->key, APK_BLOB_STRLIT("="), &m->key, &val))
			m->value_explicit = true, m->value = val.ptr;
		int r = opt_match(m);
		if (st->execute) {
			if (r != OPT_MATCH_EXACT && r != OPT_MATCH_PARTIAL) return r;
			r = m->func(ac, m->optid, m->optarg);
			if (r < 0) return r;
		}
	} else {
		for (int j = 1; arg[j]; j++) {
			*m = (struct apk_opt_match) {
				.short_opt = arg[j],
				.key = APK_BLOB_PTR_LEN(&m->short_opt, 1),
				.value = arg[j+1] ? &arg[j+1] : next_arg,
			};
			int r = opt_match(m);
			if (st->execute) {
				if (r != OPT_MATCH_EXACT && r != OPT_MATCH_PARTIAL) return r;
				r = m->func(ac, m->optid, m->optarg);
				if (r < 0) return r;
			}
			if (m->value_used) break;
		}
	}
	if (m->value_used && m->optarg == next_arg) opt_parse_next(st);
	return 0;
}

static int load_config(struct apk_ctx *ac)
{
	struct apk_out *out = &ac->out;
	struct apk_istream *is;
	apk_blob_t newline = APK_BLOB_STRLIT("\n"), comment = APK_BLOB_STRLIT("#");
	apk_blob_t space = APK_BLOB_STRLIT(" "), line, value;
	int r;

	is = apk_istream_from_file(AT_FDCWD, getenv("APK_CONFIG") ?: "/etc/apk/config");
	if (is == ERR_PTR(-ENOENT)) is = apk_istream_from_file(AT_FDCWD, "/lib/apk/config");
	if (IS_ERR(is)) return PTR_ERR(is);

	while (apk_istream_get_delim(is, newline, &line) == 0) {
		struct apk_opt_match m = {0};
		apk_blob_split(line, comment, &line, &value);
		m.key = apk_blob_trim_end(line, ' ');
		if (apk_blob_split(m.key, space, &m.key, &value)) {
			m.key = apk_blob_trim_end(m.key, ' ');
			m.value = apk_balloc_cstr(&ac->ba, value);
			m.value_explicit = true;
		}
		if (m.key.len == 0) continue;
		r = opt_match(&m);
		if (r == OPT_MATCH_AMBIGUOUS) r = OPT_MATCH_INVALID;
		if (r == OPT_MATCH_EXACT) r = m.func(ac, m.optid, m.optarg);
		if (r != 0 && apk_out_verbosity(out) >= 0) opt_print_error(r, APK_OUT_WARNING, "config", &m, out);
	}
	return apk_istream_close(is);
}

static struct apk_applet *applet_from_arg0(const char *arg0)
{
	const char *prog = apk_last_path_segment(arg0);
	if (strncmp(prog, "apk_", 4) != 0) return NULL;
	return apk_applet_find(prog + 4);
}

static int parse_options(int argc, char **argv, struct apk_string_array **args, struct apk_ctx *ac)
{
	struct apk_out *out = &ac->out;
	struct apk_opt_match m;
	bool applet_arg_pending = false;
	int r;
	char *arg;

	applet = applet_from_arg0(argv[0]);
	if (!applet) {
		for (struct opt_parse_state st = opt_parse_init(argc, argv, false); opt_parse_ok(&st); opt_parse_next(&st)) {
			if (opt_parse_argv(&st, &m, ac) != OPT_MATCH_NON_OPTION) continue;
			applet = apk_applet_find(opt_parse_arg(&st));
			if (!applet) continue;
			applet_arg_pending = true;
			break;
		}
	}
	if (applet) {
		ac->query.ser = &apk_serializer_query;
		ac->open_flags = applet->open_flags;
		if (applet->context_size) applet_ctx = calloc(1, applet->context_size);
		if (applet->parse) applet->parse(applet_ctx, &ctx, APK_OPTIONS_INIT, NULL);
	}
	load_config(ac);

	for (struct opt_parse_state st = opt_parse_init(argc, argv, true); opt_parse_ok(&st); opt_parse_next(&st)) {
		r = opt_parse_argv(&st, &m, ac);
		switch (r) {
		case 0:
			break;
		case OPT_MATCH_NON_OPTION:
			arg = opt_parse_arg(&st);
			if (applet_arg_pending && strcmp(arg, applet->name) == 0)
				applet_arg_pending = false;
			else if (arg[0] || !applet || !applet->remove_empty_arguments)
				apk_string_array_add(args, arg);
			break;
		case -ENOTSUP:
			return usage(out);
		default:
			if (r < 0) return r;
		case -EINVAL:
			opt_print_error(r, APK_OUT_ERROR, "command line", &m, out);
			return 1;
		}
	}
	return 0;
}

static void on_sigint(int s)
{
	apk_db_close(&db);
	exit(128 + s);
}

static void on_sigwinch(int s)
{
	apk_out_reset(&ctx.out);
}

static void setup_terminal(void)
{
	static char buf[200];
	setvbuf(stderr, buf, _IOLBF, sizeof buf);
	signal(SIGWINCH, on_sigwinch);
	signal(SIGPIPE, SIG_IGN);
}

static void redirect_callback(int code, const char *url)
{
	apk_warn(&ctx.out, "Permanently redirected to %s", url);
}

int main(int argc, char **argv)
{
	struct apk_out *out = &ctx.out;
	struct apk_string_array *args;
	int r;

	apk_argc = argc;
	apk_argv = argv;
	apk_string_array_init(&args);

	apk_crypto_init();
	apk_ctx_init(&ctx);
	ctx.on_tty = isatty(STDOUT_FILENO);
	ctx.interactive = (access("/etc/apk/interactive", F_OK) == 0) ? APK_AUTO : APK_NO;
	ctx.pretty_print = APK_AUTO;
	ctx.out.progress = APK_AUTO;

	umask(0);
	setup_terminal();

	apk_io_url_init(&ctx.out);
	apk_io_url_set_timeout(60);
	apk_io_url_set_redirect_callback(redirect_callback);

	r = parse_options(argc, argv, &args, &ctx);
	if (r != 0) goto err;

	if (applet == NULL) {
		if (apk_array_len(args)) {
			apk_err(out, "'%s' is not an apk command. See 'apk --help'.", args->item[0]);
			return 1;
		}
		return usage(out);
	}

	apk_db_init(&db, &ctx);
	signal(SIGINT, on_sigint);

	r = apk_ctx_prepare(&ctx);
	if (r != 0) goto err;

	apk_out_log_argv(&ctx.out, apk_argv);
	version(&ctx.out, APK_OUT_LOG_ONLY);

	if (ctx.open_flags) {
		r = apk_db_open(&db);
		if (r != 0) {
			apk_err(out, "Failed to open apk database: %s", apk_error_str(r));
			goto err;
		}
	}

	apk_io_url_set_redirect_callback(NULL);

	r = applet->main(applet_ctx, &ctx, args);
	signal(SIGINT, SIG_IGN);
	apk_db_close(&db);

err:
	if (r == -ESHUTDOWN) r = 0;
	if (applet_ctx) free(applet_ctx);

	apk_ctx_free(&ctx);
	apk_string_array_free(&args);

	if (r < 0) r = 250;
	if (r > 99) r = 99;
	return r;
}
