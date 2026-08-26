/* commit.c - Alpine Package Keeper (APK)
 * Apply solver calculated changes to database.
 *
 * Copyright (C) 2008-2013 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <unistd.h>
#include "apk_defines.h"
#include "apk_database.h"
#include "apk_package.h"
#include "apk_solver.h"
#include "apk_print.h"

#ifdef __linux__
static bool running_on_host(void)
{
	static const char expected[] = "2 (kthreadd) ";
	char buf[sizeof expected - 1];
	bool on_host = false;

	int fd = open("/proc/2/stat", O_RDONLY);
	if (fd >= 0) {
		if (read(fd, buf, sizeof buf) == sizeof buf &&
		    memcmp(buf, expected, sizeof buf) == 0)
			on_host = true;
		close(fd);
	}
	return on_host;
}
#else
static bool running_on_host(void) { return false; }
#endif

struct apk_stats {
	uint64_t bytes;
	unsigned int changes;
	unsigned int packages;
};

struct progress {
	struct apk_progress prog;
	struct apk_stats done;
	struct apk_stats total;
	struct apk_package *pkg;
	int total_changes_digits;
};

static inline bool pkg_available(struct apk_database *db, struct apk_package *pkg)
{
	return (pkg->cached || pkg->filename_ndx || apk_db_pkg_available(db, pkg)) ? true : false;
}

static bool print_change(struct apk_database *db, struct apk_change *change, struct progress *prog)
{
	struct apk_out *out = &db->ctx->out;
	struct apk_name *name;
	struct apk_package *oldpkg = change->old_pkg;
	struct apk_package *newpkg = change->new_pkg;
	const char *msg = NULL, *status;
	char statusbuf[32];
	apk_blob_t *oneversion = NULL;
	int r;

	status = apk_fmts(statusbuf, sizeof statusbuf, "(%*i/%i)",
		prog->total_changes_digits, prog->done.changes+1,
		prog->total.changes) ?: "(?)";

	name = newpkg ? newpkg->name : oldpkg->name;
	if (oldpkg == NULL) {
		msg = "Installing";
		oneversion = newpkg->version;
	} else if (newpkg == NULL) {
		msg = "Purging";
		oneversion = oldpkg->version;
	} else if (newpkg == oldpkg) {
		if (change->reinstall) {
			if (pkg_available(db, newpkg))
				msg = "Reinstalling";
			else
				msg = "[APK unavailable, skipped] Reinstalling";
		} else if (change->old_repository_tag != change->new_repository_tag) {
			msg = "Updating pinning";
		}
		oneversion = newpkg->version;
	} else {
		r = apk_pkg_version_compare(newpkg, oldpkg);
		switch (r) {
		case APK_VERSION_LESS:
			msg = "Downgrading";
			break;
		case APK_VERSION_EQUAL:
			msg = "Replacing";
			break;
		case APK_VERSION_GREATER:
			msg = "Upgrading";
			break;
		}
	}
	if (!msg) return false;

	if (oneversion) {
		apk_msg(out, "%s %s %s" BLOB_FMT " (" BLOB_FMT ")",
			status, msg,
			name->name,
			BLOB_PRINTF(db->repo_tags[change->new_repository_tag].tag),
			BLOB_PRINTF(*oneversion));
	} else {
		apk_msg(out, "%s %s %s" BLOB_FMT " (" BLOB_FMT " -> " BLOB_FMT ")",
			status, msg,
			name->name,
			BLOB_PRINTF(db->repo_tags[change->new_repository_tag].tag),
			BLOB_PRINTF(*oldpkg->version),
			BLOB_PRINTF(*newpkg->version));
	}
	return true;
}

static uint64_t change_size(struct apk_change *change)
{
	if (change->new_pkg) return change->new_pkg->size;
	return change->old_pkg->size / 16;
}

static void count_change(struct apk_change *change, struct apk_stats *stats)
{
	if (change->new_pkg != change->old_pkg || change->reinstall)
		stats->bytes += change_size(change);
	else if (change->new_repository_tag == change->old_repository_tag)
		return;
	stats->packages++;
	stats->changes++;
}

static int dump_packages(struct apk_database *db, struct apk_change_array *changes,
			 int (*cmp)(struct apk_change *change),
			 bool details, const char *msg)
{
	struct apk_out *out = &db->ctx->out;
	struct apk_name *name;
	struct apk_indent indent;
	int match = 0;

	apk_print_indented_init(&indent, out, 0);
	apk_array_foreach(change, changes) {
		if (!cmp(change)) continue;
		if (!match) apk_print_indented_group(&indent, 2, "%s:\n", msg);
		if (change->new_pkg != NULL)
			name = change->new_pkg->name;
		else
			name = change->old_pkg->name;

		if (details) {
			if (!change->reinstall && change->new_pkg && change->old_pkg) {
				apk_out(out, "  %s" BLOB_FMT " (" BLOB_FMT " -> " BLOB_FMT ")",
					name->name,
					BLOB_PRINTF(db->repo_tags[change->new_repository_tag].tag),
					BLOB_PRINTF(*change->old_pkg->version),
					BLOB_PRINTF(*change->new_pkg->version));
			} else {
				apk_out(out, "  %s" BLOB_FMT " (" BLOB_FMT ")",
					name->name,
					BLOB_PRINTF(db->repo_tags[change->new_repository_tag].tag),
					BLOB_PRINTF(change->old_pkg ? *change->old_pkg->version : *change->new_pkg->version));
			}
		} else {
			apk_print_indented(&indent, APK_BLOB_STR(name->name));
		}
		match++;
	}
	apk_print_indented_end(&indent);
	return match;
}

static int sort_change(const void *a, const void *b)
{
	const struct apk_change *ca = a;
	const struct apk_change *cb = b;
	const struct apk_name *na = ca->old_pkg ? ca->old_pkg->name : ca->new_pkg->name;
	const struct apk_name *nb = cb->old_pkg ? cb->old_pkg->name : cb->new_pkg->name;
	return apk_name_cmp_display(na, nb);
}

static int cmp_remove(struct apk_change *change)
{
	return change->new_pkg == NULL;
}

static int cmp_new(struct apk_change *change)
{
	return change->old_pkg == NULL;
}

static int cmp_reinstall(struct apk_change *change)
{
	return change->reinstall;
}

static int cmp_non_repository_verbose(struct apk_change *change)
{
	if (!change->new_pkg || change->new_pkg->name->has_repository_providers) return 0;
	return 1;
}

static int cmp_non_repository(struct apk_change *change)
{
	if (!cmp_non_repository_verbose(change)) return 0;
	if (change->new_pkg->name->name[0] == '.') return 0;
	return 1;
}

static int cmp_downgrade(struct apk_change *change)
{
	if (change->new_pkg == NULL || change->old_pkg == NULL)
		return 0;
	if (apk_pkg_version_compare(change->new_pkg, change->old_pkg)
	    & APK_VERSION_LESS)
		return 1;
	return 0;
}

static int cmp_upgrade(struct apk_change *change)
{
	if (change->new_pkg == NULL || change->old_pkg == NULL)
		return 0;

	/* Count swapping package as upgrade too - this can happen if
	 * same package version is used after it was rebuilt against
	 * newer libraries. Basically, different (and probably newer)
	 * package, but equal version number. */
	if ((apk_pkg_version_compare(change->new_pkg, change->old_pkg) &
	     (APK_VERSION_GREATER | APK_VERSION_EQUAL)) &&
	    (change->new_pkg != change->old_pkg))
		return 1;

	return 0;
}

static int run_triggers(struct apk_database *db, struct apk_changeset *changeset)
{
	struct apk_installed_package *ipkg;
	int errors = 0;

	if (apk_db_fire_triggers(db) == 0)
		return 0;

	apk_array_foreach(change, changeset->changes) {
		struct apk_package *pkg = change->new_pkg;
		if (pkg == NULL)
			continue;
		ipkg = pkg->ipkg;
		if (ipkg == NULL || apk_array_len(ipkg->pending_triggers) == 0)
			continue;

		apk_string_array_add(&ipkg->pending_triggers, NULL);
		errors += apk_ipkg_run_script(ipkg, db, APK_SCRIPT_TRIGGER,
					      ipkg->pending_triggers->item) != 0;
		apk_string_array_free(&ipkg->pending_triggers);
	}
	return errors;
}

#define PRE_COMMIT_HOOK		0
#define POST_COMMIT_HOOK	1

struct apk_commit_hook {
	struct apk_database *db;
	int type;
};

static int run_commit_hook(void *ctx, int dirfd, const char *path, const char *file)
{
	static char *const commit_hook_str[] = { "pre-commit", "post-commit" };
	struct apk_commit_hook *hook = (struct apk_commit_hook *) ctx;
	struct apk_database *db = hook->db;
	struct apk_out *out = &db->ctx->out;
	char buf[PATH_MAX], fn[PATH_MAX], *argv[] = { fn, (char *) commit_hook_str[hook->type], NULL };
	const char *linepfx;
	int ret = 0;

	if (file[0] == '.') return 0;
	if ((db->ctx->flags & (APK_NO_SCRIPTS | APK_SIMULATE)) != 0) return 0;
	if (apk_fmt(fn, sizeof fn, "%s/%s", path, file) < 0) return 0;

	if ((db->ctx->flags & APK_NO_COMMIT_HOOKS) != 0) {
		apk_msg(out, "Skipping: %s %s", fn, commit_hook_str[hook->type]);
		return 0;
	}

	if (apk_out_verbosity(out) >= 2) {
		apk_dbg(out, "Executing /%s %s", fn, commit_hook_str[hook->type]);
		linepfx = "* ";
	} else {
		apk_out_progress_note(out, "executing %s %s", commit_hook_str[hook->type], file);
		linepfx = apk_fmts(buf, sizeof buf, "Executing %s %s\n* ", commit_hook_str[hook->type], file);
	}

	if (apk_db_run_script(db, commit_hook_str[hook->type], NULL, -1, argv, linepfx) < 0 && hook->type == PRE_COMMIT_HOOK)
		ret = -2;

	return ret;
}

static int run_commit_hooks(struct apk_database *db, int type)
{
	struct apk_commit_hook hook = { .db = db, .type = type };
	return apk_dir_foreach_config_file(db->root_fd,
		run_commit_hook, &hook, apk_filename_is_hidden,
		"etc/apk/commit_hooks.d",
		"lib/apk/commit_hooks.d",
		NULL);
}

static void sync_if_needed(struct apk_database *db)
{
	struct apk_ctx *ac = db->ctx;
	if (ac->flags & APK_SIMULATE) return;
	if (ac->sync == APK_NO) return;
	if (ac->sync == APK_AUTO && (ac->root_set || db->usermode || !running_on_host())) return;
	apk_out_progress_note(&ac->out, "syncing disks...");
	sync();
}

static int calc_precision(unsigned int num)
{
	int precision = 1;
	while (num >= 10) {
		precision++;
		num /= 10;
	}
	return precision;
}

int apk_solver_precache_changeset(struct apk_database *db, struct apk_changeset *changeset, bool changes_only)
{
	struct progress prog = { 0 };
	struct apk_out *out = &db->ctx->out;
	struct apk_package *pkg;
	struct apk_repository *repo;
	int r, errors = 0;

	apk_array_foreach(change, changeset->changes) {
		pkg = change->new_pkg;
		if (changes_only && pkg == change->old_pkg) continue;
		if (!pkg || pkg->cached || (pkg->repos & db->local_repos) || !pkg->installed_size) continue;
		if (!apk_db_select_repo(db, pkg)) continue;
		prog.total.bytes += pkg->size;
		prog.total.packages++;
		prog.total.changes++;
	}
	if (!prog.total.packages) return 0;

	prog.total_changes_digits = calc_precision(prog.total.packages);
	apk_msg(out, "Downloading %d packages...", prog.total.packages);

	apk_progress_start(&prog.prog, out, "download", apk_progress_weight(prog.total.bytes, prog.total.packages));
	apk_array_foreach(change, changeset->changes) {
		pkg = change->new_pkg;
		if (changes_only && pkg == change->old_pkg) continue;
		if (!pkg || pkg->cached || (pkg->repos & db->local_repos) || !pkg->installed_size) continue;
		if (!(repo = apk_db_select_repo(db, pkg))) continue;

		apk_msg(out, "(%*i/%i) Downloading " PKG_VER_FMT,
			prog.total_changes_digits, prog.done.packages+1,
			prog.total.packages,
			PKG_VER_PRINTF(pkg));

		apk_progress_item_start(&prog.prog, apk_progress_weight(prog.done.bytes, prog.done.packages), pkg->size);
		r = apk_cache_download(db, repo, pkg, &prog.prog);
		if (r && r != -APKE_FILE_UNCHANGED) {
			apk_err(out, PKG_VER_FMT ": %s", PKG_VER_PRINTF(pkg), apk_error_str(r));
			errors++;
		}
		apk_progress_item_end(&prog.prog);
		prog.done.bytes += pkg->size;
		prog.done.packages++;
		prog.done.changes++;
	}
	apk_progress_end(&prog.prog);

	if (errors) return -errors;
	return prog.done.packages;
}

int apk_solver_commit_changeset(struct apk_database *db,
				struct apk_changeset *changeset,
				struct apk_dependency_array *world)
{
	struct apk_out *out = &db->ctx->out;
	struct progress prog = { 0 };
	char buf[64];
	apk_blob_t humanized;
	uint64_t download_size = 0;
	int64_t size_diff = 0;
	int r, errors = 0, pkg_diff = 0;

	assert(world);
	if (apk_db_check_world(db, world) != 0) {
		apk_err(out, "Not committing changes due to missing repository tags.");
		return -1;
	}

	if (changeset->changes == NULL)
		goto all_done;

	/* Count what needs to be done */
	apk_array_foreach(change, changeset->changes) {
		count_change(change, &prog.total);
		if (change->new_pkg) {
			size_diff += change->new_pkg->installed_size;
			pkg_diff++;
			if (change->new_pkg != change->old_pkg &&
			    !(change->new_pkg->repos & db->local_repos))
				download_size += change->new_pkg->size;
		}
		if (change->old_pkg) {
			if (change->old_pkg != change->new_pkg)
				change->old_pkg->ipkg->to_be_removed = 1;
			size_diff -= change->old_pkg->installed_size;
			pkg_diff--;
		}
	}
	prog.total_changes_digits = calc_precision(prog.total.changes);

	if (apk_out_verbosity(out) > 1 || db->ctx->interactive) {
		struct apk_change_array *sorted;
		bool details = apk_out_verbosity(out) >= 2;

		apk_change_array_init(&sorted);
		apk_change_array_copy(&sorted, changeset->changes);
		apk_array_qsort(sorted, sort_change);

		dump_packages(db, sorted, details ? cmp_non_repository_verbose : cmp_non_repository, false,
			"NOTE: Consider running apk upgrade with --prune and/or --available.\n"
			"The following packages are no longer available from a repository");
		r = dump_packages(db, sorted, cmp_remove, details,
			"The following packages will be REMOVED");
		r += dump_packages(db, sorted, cmp_downgrade, details,
			"The following packages will be DOWNGRADED");
		if (r || db->ctx->interactive || apk_out_verbosity(out) > 2) {
			r += dump_packages(db, sorted, cmp_new, details,
				"The following NEW packages will be installed");
			r += dump_packages(db, sorted, cmp_upgrade, details,
				"The following packages will be upgraded");
			r += dump_packages(db, sorted, cmp_reinstall, details,
				"The following packages will be reinstalled");
			if (download_size) {
				humanized = apk_fmt_human_size(buf, sizeof buf, download_size, 1);
				apk_msg(out, "Need to download " BLOB_FMT " of packages.", BLOB_PRINTF(humanized));
			}
			humanized = apk_fmt_human_size(buf, sizeof buf, llabs(size_diff), 1);
			apk_msg(out, "After this operation, " BLOB_FMT " of %s.",
				BLOB_PRINTF(humanized), (size_diff < 0) ?
				"disk space will be freed" :
				"additional disk space will be used");
		}
		apk_change_array_free(&sorted);

		if (r > 0 && db->ctx->interactive && !(db->ctx->flags & APK_SIMULATE)) {
			printf("Do you want to continue [Y/n]? ");
			fflush(stdout);
			r = fgetc(stdin);
			if (r != 'y' && r != 'Y' && r != '\n' && r != EOF)
				return -1;
		}
	}

	if (db->ctx->cache_predownload && apk_db_cache_active(db)) {
		r = apk_solver_precache_changeset(db, changeset, true);
		if (r < 0) return -1;
		if (r > 0) apk_msg(out, "Proceeding with upgrade...");
	}

	if (run_commit_hooks(db, PRE_COMMIT_HOOK) == -2)
		return -1;

	/* Go through changes */
	db->indent_level = 1;
	apk_progress_start(&prog.prog, out, "install", apk_progress_weight(prog.total.bytes, prog.total.packages));
	apk_array_foreach(change, changeset->changes) {
		r = change->old_pkg &&
			(change->old_pkg->ipkg->broken_files ||
			 change->old_pkg->ipkg->broken_script);
		if (print_change(db, change, &prog)) {
			prog.pkg = change->new_pkg ?: change->old_pkg;
			if (change->old_pkg != change->new_pkg || (change->reinstall && pkg_available(db, change->new_pkg))) {
				apk_progress_item_start(&prog.prog, apk_progress_weight(prog.done.bytes, prog.done.packages), change_size(change));
				if (!(db->ctx->flags & APK_SIMULATE))
					r = apk_db_install_pkg(db, change->old_pkg, change->new_pkg, &prog.prog) != 0;
				apk_progress_item_end(&prog.prog);
			}
			if (change->new_pkg && change->new_pkg->ipkg)
				change->new_pkg->ipkg->repository_tag = change->new_repository_tag;
		}
		errors += r;
		count_change(change, &prog.done);
	}
	apk_progress_end(&prog.prog);
	db->indent_level = 0;

	errors += db->num_dir_update_errors;
	errors += run_triggers(db, changeset);

all_done:
	apk_dependency_array_copy(&db->world, world);
	if (apk_db_write_config(db) != 0) errors++;
	run_commit_hooks(db, POST_COMMIT_HOOK);

	if (!db->performing_preupgrade) {
		char buf2[32];
		const char *msg = "OK:";

		sync_if_needed(db);

		if (errors) msg = apk_fmts(buf2, sizeof buf2, "%d error%s;",
				errors, errors > 1 ? "s" : "") ?: "ERRORS;";

		uint64_t installed_bytes = db->installed.stats.bytes;
		int installed_packages = db->installed.stats.packages;
		if (db->ctx->flags & APK_SIMULATE) {
			installed_bytes += size_diff;
			installed_packages += pkg_diff;
		}

		humanized = apk_fmt_human_size(buf, sizeof buf, installed_bytes, 1);

		if (apk_out_verbosity(out) > 1) {
			apk_msg(out, "%s %d packages, %d dirs, %d files, " BLOB_FMT,
				msg,
				installed_packages,
				db->installed.stats.dirs,
				db->installed.stats.files,
				BLOB_PRINTF(humanized)
				);
		} else {
			apk_msg(out, "%s " BLOB_FMT " in %d packages",
				msg,
				BLOB_PRINTF(humanized),
				installed_packages);
		}
	}
	return errors;
}

enum {
	STATE_PRESENT		= 0x80000000,
	STATE_MISSING		= 0x40000000,
	STATE_VIRTUAL_ONLY	= 0x20000000,
	STATE_INSTALLIF		= 0x10000000,
	STATE_COUNT_MASK	= 0x0000ffff,
};

struct print_state {
	struct apk_database *db;
	struct apk_dependency_array *world;
	struct apk_indent i;
	const char *label;
	int num_labels;
	int match;
};

static void label_start(struct print_state *ps, const char *text)
{
	if (ps->label) {
		apk_print_indented_line(&ps->i, "  %s:\n", ps->label);
		ps->label = NULL;
		ps->num_labels++;
	}
	if (!ps->i.x) apk_print_indented_group(&ps->i, 0, "    %s", text);
}
static void label_end(struct print_state *ps)
{
	apk_print_indented_end(&ps->i);
}

static void print_pinning_errors(struct print_state *ps, struct apk_package *pkg, unsigned int tag)
{
	struct apk_database *db = ps->db;
	int i;

	if (pkg->ipkg != NULL)
		return;

	if (!apk_db_pkg_available(db, pkg) && !pkg->cached && !pkg->filename_ndx) {
		label_start(ps, "masked in:");
		apk_print_indented_fmt(&ps->i, "--no-network");
	} else if (!(BIT(pkg->layer) & db->active_layers)) {
		label_start(ps, "masked in:");
		apk_print_indented_fmt(&ps->i, "layer");
	} else if (!pkg->repos && pkg->cached) {
		label_start(ps, "masked in:");
		apk_print_indented_fmt(&ps->i, "cache");
	} else {
		if (pkg->repos & apk_db_get_pinning_mask_repos(db, APK_DEFAULT_PINNING_MASK | BIT(tag)))
			return;
		for (i = 0; i < db->num_repo_tags; i++) {
			if (pkg->repos & db->repo_tags[i].allowed_repos) {
				label_start(ps, "masked in:");
				apk_print_indented(&ps->i, db->repo_tags[i].tag);
			}
		}
	}
	label_end(ps);
}

static void print_conflicts(struct print_state *ps, struct apk_package *pkg)
{
	int once;

	apk_array_foreach(p, pkg->name->providers) {
		if (p->pkg == pkg || !p->pkg->marked)
			continue;
		label_start(ps, "conflicts:");
		apk_print_indented_fmt(&ps->i, PKG_VER_FMT, PKG_VER_PRINTF(p->pkg));
	}
	apk_array_foreach(d, pkg->provides) {
		once = 1;
		apk_array_foreach(p, d->name->providers) {
			if (!p->pkg->marked)
				continue;
			if (d->version == &apk_atom_null &&
			    p->version == &apk_atom_null)
				continue;
			if (once && p->pkg == pkg &&
			    p->version == d->version) {
				once = 0;
				continue;
			}
			label_start(ps, "conflicts:");
			apk_print_indented_fmt(
				&ps->i, PKG_VER_FMT "[" DEP_FMT "]",
				PKG_VER_PRINTF(p->pkg),
				DEP_PRINTF(d));
		}
	}
	label_end(ps);
}

struct matched_dep {
	struct apk_package *pkg;
	struct apk_dependency *dep;
};
APK_ARRAY(matched_dep_array, struct matched_dep);

static void match_dep(struct apk_package *pkg0, struct apk_dependency *d0, struct apk_package *pkg, void *ctx)
{
	struct matched_dep_array **deps = ctx;
	matched_dep_array_add(deps, (struct matched_dep) {
		.pkg = pkg0,
		.dep = d0,
	});
}

static int matched_dep_sort(const void *p1, const void *p2)
{
	const struct matched_dep *m1 = p1, *m2 = p2;
	int r;

	if (m1->pkg && m2->pkg) {
		r = apk_pkg_cmp_display(m1->pkg, m2->pkg);
		if (r != 0) return r;
	}
	return m1->dep->op - m2->dep->op;
}

static void print_mdeps(struct print_state *ps, const char *label, struct matched_dep_array *deps)
{
	struct apk_database *db = ps->db;

	if (apk_array_len(deps) == 0) return;

	label_start(ps, label);
	apk_array_qsort(deps, matched_dep_sort);
	apk_array_foreach(dep, deps) {
		if (dep->pkg == NULL)
			apk_print_indented_fmt(&ps->i, "world[" DEP_FMT BLOB_FMT "]", DEP_PRINTF(dep->dep),
				BLOB_PRINTF(db->repo_tags[dep->dep->repository_tag].tag));
		else
			apk_print_indented_fmt(&ps->i, PKG_VER_FMT "[" DEP_FMT "]",
					       PKG_VER_PRINTF(dep->pkg),
					       DEP_PRINTF(dep->dep));
	}
	apk_array_reset(deps);
}

static void print_deps(struct print_state *ps, struct apk_package *pkg, int match)
{
	const char *label = (match & APK_DEP_SATISFIES) ? "satisfies:" : "breaks:";
	struct matched_dep_array *deps;

	matched_dep_array_init(&deps);

	ps->match = match;
	match |= APK_FOREACH_MARKED | APK_FOREACH_DEP;
	apk_pkg_foreach_matching_dependency(NULL, ps->world, match|apk_foreach_genid(), pkg, match_dep, &deps);
	print_mdeps(ps, label, deps);
	apk_pkg_foreach_reverse_dependency(pkg, match|apk_foreach_genid(), match_dep, &deps);
	print_mdeps(ps, label, deps);
	label_end(ps);

	matched_dep_array_free(&deps);
}

static void print_broken_deps(struct print_state *ps, struct apk_dependency_array *deps, const char *label)
{
	apk_array_foreach(dep, deps) {
		if (!dep->broken) continue;
		label_start(ps, label);
		apk_print_indented_fmt(&ps->i, DEP_FMT, DEP_PRINTF(dep));
	}
	label_end(ps);
}

static void analyze_package(struct print_state *ps, struct apk_package *pkg, unsigned int tag)
{
	char pkgtext[PKG_VER_MAX];

	ps->label = apk_fmts(pkgtext, sizeof pkgtext, PKG_VER_FMT, PKG_VER_PRINTF(pkg));

	if (pkg->uninstallable) {
		label_start(ps, "error:");
		apk_print_indented_fmt(&ps->i, "uninstallable");
		label_end(ps);
		if (!apk_db_arch_compatible(ps->db, pkg->arch)) {
			label_start(ps, "arch:");
			apk_print_indented_fmt(&ps->i, BLOB_FMT, BLOB_PRINTF(*pkg->arch));
			label_end(ps);
		}
		print_broken_deps(ps, pkg->depends, "depends:");
		print_broken_deps(ps, pkg->provides, "provides:");
		print_broken_deps(ps, pkg->install_if, "install_if:");
	}

	print_pinning_errors(ps, pkg, tag);
	print_conflicts(ps, pkg);
	print_deps(ps, pkg, APK_DEP_CONFLICTS);
	if (ps->label == NULL)
		print_deps(ps, pkg, APK_DEP_SATISFIES);
}

static void analyze_missing_name(struct print_state *ps, struct apk_name *name)
{
	struct apk_database *db = ps->db;
	char label[256];
	unsigned int genid;
	int refs;

	if (apk_array_len(name->providers) != 0) {
		ps->label = apk_fmts(label, sizeof label, "%s (virtual)", name->name);

		label_start(ps, "note:");
		apk_print_indented_words(&ps->i, "please select one of the 'provided by' packages explicitly");
		label_end(ps);

		label_start(ps, "provided by:");
		apk_array_foreach(p0, name->providers)
			p0->pkg->name->state_int++;
		apk_array_foreach(p0, name->providers) {
			struct apk_name *name0 = p0->pkg->name;
			refs = (name0->state_int & STATE_COUNT_MASK);
			if (refs == apk_array_len(name0->providers)) {
				/* name only */
				apk_print_indented(&ps->i, APK_BLOB_STR(name0->name));
				name0->state_int &= ~STATE_COUNT_MASK;
			} else if (refs > 0) {
				/* individual package */
				apk_print_indented_fmt(&ps->i, PKG_VER_FMT, PKG_VER_PRINTF(p0->pkg));
				name0->state_int--;
			}
		}
		label_end(ps);
	} else {
		ps->label = apk_fmts(label, sizeof label, "%s (no such package)", name->name);
	}

	label_start(ps, "required by:");
	apk_array_foreach(d0, ps->world) {
		if (d0->name != name || apk_dep_conflict(d0)) continue;
		apk_print_indented_fmt(&ps->i, "world[" DEP_FMT BLOB_FMT "]",
			DEP_PRINTF(d0),
			BLOB_PRINTF(db->repo_tags[d0->repository_tag].tag));
	}
	genid = apk_foreach_genid();
	apk_array_foreach_item(name0, name->rdepends) {
		apk_array_foreach(p0, name0->providers) {
			if (!p0->pkg->marked) continue;
			if (p0->pkg->foreach_genid == genid) continue;
			p0->pkg->foreach_genid = genid;
			apk_array_foreach(d0, p0->pkg->depends) {
				if (d0->name != name || apk_dep_conflict(d0)) continue;
				apk_print_indented_fmt(&ps->i,
					PKG_VER_FMT "[" DEP_FMT "]",
					PKG_VER_PRINTF(p0->pkg),
					DEP_PRINTF(d0));
				goto next_name;
			}
		}
		next_name:;
	}
	label_end(ps);
}

static void analyze_deps(struct print_state *ps, struct apk_dependency_array *deps)
{
	apk_array_foreach(d0, deps) {
		struct apk_name *name0 = d0->name;
		if (apk_dep_conflict(d0)) continue;
		if ((name0->state_int & (STATE_INSTALLIF | STATE_PRESENT | STATE_MISSING)) != 0)
			continue;
		name0->state_int |= STATE_MISSING;
		analyze_missing_name(ps, name0);
	}
}

static void discover_deps(struct apk_dependency_array *deps);
static void discover_name(struct apk_name *name, int pkg_state);

static void discover_reverse_iif(struct apk_name *name)
{
	apk_array_foreach_item(name0, name->rinstall_if) {
		apk_array_foreach(p, name0->providers) {
			int ok = 1;
			if (!p->pkg->marked) continue;
			if (apk_array_len(p->pkg->install_if) == 0) continue;
			apk_array_foreach(d, p->pkg->install_if) {
				if (apk_dep_conflict(d) == !!(d->name->state_int & (STATE_PRESENT|STATE_INSTALLIF))) {
					ok = 0;
					break;
				}
			}
			if (ok) {
				discover_name(p->pkg->name, STATE_INSTALLIF);
				apk_array_foreach(d, p->pkg->provides)
					discover_name(d->name, STATE_INSTALLIF);
			}
		}
	}
}

static int is_name_concrete(struct apk_package *pkg, struct apk_name *name)
{
	if (pkg->name == name) return 1;
	apk_array_foreach(d, pkg->provides) {
		if (d->name != name) continue;
		if (d->version == &apk_atom_null) continue;
		return 1;
	}
	return 0;
}

static void discover_name(struct apk_name *name, int pkg_state)
{
	apk_array_foreach(p, name->providers) {
		int state = pkg_state;
		if (!p->pkg->marked) continue;
		if ((state == STATE_PRESENT || state == STATE_INSTALLIF) &&
		    !p->pkg->provider_priority && !is_name_concrete(p->pkg, name))
			state = STATE_VIRTUAL_ONLY;
		if (p->pkg->state_int & state) continue;
		p->pkg->state_int |= state;

		p->pkg->name->state_int |= state;
		apk_array_foreach(d, p->pkg->provides) {
			int dep_state = state;
			if (dep_state == STATE_INSTALLIF && d->version == &apk_atom_null)
				dep_state = STATE_VIRTUAL_ONLY;
			d->name->state_int |= dep_state;
		}

		discover_deps(p->pkg->depends);
		if (state == STATE_PRESENT || state == STATE_INSTALLIF) {
			discover_reverse_iif(p->pkg->name);
			apk_array_foreach(d, p->pkg->provides)
				discover_reverse_iif(d->name);
		}
	}
}

static void discover_deps(struct apk_dependency_array *deps)
{
	apk_array_foreach(d, deps) {
		if (apk_dep_conflict(d)) continue;
		discover_name(d->name, STATE_PRESENT);
	}
}

void apk_solver_print_errors(struct apk_database *db,
			     struct apk_changeset *changeset,
			     struct apk_dependency_array *world)
{
	struct apk_out *out = &db->ctx->out;
	struct print_state ps;

	/* ERROR: unsatisfiable dependencies:
	 *   name:
	 *     required by: a b c d e
	 *     not available in any repository
	 *   name (virtual):
	 *     required by: a b c d e
	 *     provided by: foo bar zed
	 *   pkg-1.2:
	 *     masked by: @testing
	 *     satisfies: a[pkg]
	 *     conflicts: pkg-2.0 foo-1.2 bar-1.2
	 *     breaks: b[pkg>2] c[foo>2] d[!pkg]
	 *
	 * When two packages provide same name 'foo':
	 *   a-1:
	 *     satisfies: world[a]
	 *     conflicts: b-1[foo]
	 *   b-1:
	 *     satisfies: world[b]
	 *     conflicts: a-1[foo]
	 *
	 *   c-1:
	 *     satisfies: world[a]
	 *     conflicts: c-1[foo]  (self-conflict by providing foo twice)
	 *
	 * When two packages get pulled in:
	 *   a-1:
	 *     satisfies: app1[so:a.so.1]
	 *     conflicts: a-2
	 *   a-2:
	 *     satisfies: app2[so:a.so.2]
	 *     conflicts: a-1
	 *
	 * satisfies lists all dependencies that is not satisfiable by
	 * any other selected version. or all of them with -v.
	 */

	/* Construct information about names */
	apk_array_foreach(change, changeset->changes) {
		struct apk_package *pkg = change->new_pkg;
		if (pkg) pkg->marked = 1;
	}
	discover_deps(world);

	/* Analyze is package, and missing names referred to */
	ps = (struct print_state) {
		.db = db,
		.world = world,
	};
	apk_err(out, "unable to select packages:");
	apk_print_indented_init(&ps.i, out, 1);
	analyze_deps(&ps, world);
	apk_array_foreach(change, changeset->changes) {
		struct apk_package *pkg = change->new_pkg;
		if (!pkg) continue;
		analyze_package(&ps, pkg, change->new_repository_tag);
		analyze_deps(&ps, pkg->depends);
	}

	if (!ps.num_labels)
		apk_print_indented_line(&ps.i, "Huh? Error reporter did not find the broken constraints.\n");
}

int apk_solver_commit(struct apk_database *db,
		      unsigned short solver_flags,
		      struct apk_dependency_array *world)
{
	struct apk_out *out = &db->ctx->out;
	struct apk_changeset changeset = {};
	int r;

	if (apk_db_check_world(db, world) != 0) {
		apk_err(out, "Not committing changes due to missing repository tags.");
		return -1;
	}

	apk_change_array_init(&changeset.changes);
	r = apk_solver_solve(db, solver_flags, world, &changeset);
	if (r == 0)
		r = apk_solver_commit_changeset(db, &changeset, world);
	else
		apk_solver_print_errors(db, &changeset, world);
	apk_change_array_free(&changeset.changes);
	return r;
}
