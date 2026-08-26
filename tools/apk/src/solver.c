/* solver.c - Alpine Package Keeper (APK)
 * Up- and down-propagating, forwarding checking, deductive dependency solver.
 *
 * Copyright (C) 2008-2013 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <stdint.h>
#include <unistd.h>
#include <strings.h>
#include "apk_defines.h"
#include "apk_database.h"
#include "apk_package.h"
#include "apk_solver.h"

#include "apk_print.h"

//#define DEBUG_PRINT

#ifdef DEBUG_PRINT
#include <stdio.h>
#define dbg_printf(args...) fprintf(stderr, args)
#else
#define dbg_printf(args...)
#endif

#define ASSERT(cond, fmt...)	if (!(cond)) { apk_error(fmt); *(char*)NULL = 0; }

struct apk_solver_state {
	struct apk_database *db;
	struct apk_changeset *changeset;
	struct list_head dirty_head;
	struct list_head unresolved_head;
	struct list_head selectable_head;
	struct list_head resolvenow_head;
	unsigned int errors;
	unsigned int solver_flags_inherit;
	unsigned int pinning_inherit;
	unsigned int default_repos;
	unsigned int order_id;
	unsigned ignore_conflict : 1;
};

static struct apk_provider provider_none = {
	.pkg = NULL,
	.version = &apk_atom_null
};

void apk_solver_set_name_flags(struct apk_name *name,
			       unsigned short solver_flags,
			       unsigned short solver_flags_inheritable)
{
	name->solver_flags_set = 1;
	apk_array_foreach(p, name->providers) {
		struct apk_package *pkg = p->pkg;
		dbg_printf("marking '" PKG_VER_FMT "' = 0x%04x / 0x%04x\n",
			PKG_VER_PRINTF(pkg), solver_flags, solver_flags_inheritable);
		pkg->ss.solver_flags |= solver_flags;
		pkg->ss.solver_flags_inheritable |= solver_flags_inheritable;
	}
}

static int get_tag(struct apk_database *db, unsigned int pinning_mask, unsigned int repos)
{
	int i;

	for (i = 0; i < db->num_repo_tags; i++) {
		if (!(BIT(i) & pinning_mask))
			continue;
		if (db->repo_tags[i].allowed_repos & repos)
			return i;
	}
	return APK_DEFAULT_REPOSITORY_TAG;
}

static unsigned int get_pkg_repos(struct apk_database *db, struct apk_package *pkg)
{
	return pkg->repos | (pkg->ipkg ? db->repo_tags[pkg->ipkg->repository_tag].allowed_repos : 0);
}

static void mark_error(struct apk_solver_state *ss, struct apk_package *pkg, const char *reason)
{
	if (pkg == NULL || pkg->ss.error)
		return;
	dbg_printf("ERROR PKG: %s: %s\n", pkg->name->name, reason);
	pkg->ss.error = 1;
	ss->errors++;
}

static void queue_dirty(struct apk_solver_state *ss, struct apk_name *name)
{
	if (list_hashed(&name->ss.dirty_list) || name->ss.locked ||
	    (name->ss.requirers == 0 && !name->ss.reevaluate_iif))
		return;

	dbg_printf("queue_dirty: %s\n", name->name);
	list_add_tail(&name->ss.dirty_list, &ss->dirty_head);
}

static bool queue_resolvenow(struct apk_name *name)
{
	return name->ss.reverse_deps_done && name->ss.requirers &&
		name->ss.has_auto_selectable && !name->ss.has_options;
}

static void queue_insert(struct list_head *head, struct apk_name *name)
{
	struct apk_name *name0;

	list_for_each_entry(name0, head, ss.unresolved_list) {
		if (name->ss.order_id < name0->ss.order_id) continue;
		list_add_before(&name->ss.unresolved_list, &name0->ss.unresolved_list);
		return;
	}
	list_add_tail(&name->ss.unresolved_list, head);
}

static void queue_unresolved(struct apk_solver_state *ss, struct apk_name *name, bool reevaluate)
{
	if (name->ss.locked) return;
	if (list_hashed(&name->ss.unresolved_list)) {
		if (name->ss.resolvenow) return;
		if (queue_resolvenow(name) == 1)
			name->ss.resolvenow = 1;
		else if (!reevaluate)
			return;
		list_del_init(&name->ss.unresolved_list);
	} else {
		if (name->ss.requirers == 0 && !name->ss.has_iif && !name->ss.iif_needed) return;
		name->ss.resolvenow = queue_resolvenow(name);
	}

	dbg_printf("queue_unresolved: %s, requirers=%d, has_iif=%d, resolvenow=%d\n",
		name->name, name->ss.requirers, name->ss.has_iif, name->ss.resolvenow);
	if (name->ss.resolvenow) {
		list_add_tail(&name->ss.unresolved_list, &ss->resolvenow_head);
		return;
	}
	queue_insert(name->ss.has_auto_selectable ? &ss->selectable_head : &ss->unresolved_head, name);
}

static void reevaluate_reverse_deps(struct apk_solver_state *ss, struct apk_name *name)
{
	apk_array_foreach_item(name0, name->rdepends) {
		if (!name0->ss.seen) continue;
		name0->ss.reevaluate_deps = 1;
		queue_dirty(ss, name0);
	}
}

static void reevaluate_reverse_installif(struct apk_solver_state *ss, struct apk_name *name)
{
	apk_array_foreach_item(name0, name->rinstall_if) {
		if (!name0->ss.seen) continue;
		if (name0->ss.no_iif) continue;
		name0->ss.reevaluate_iif = 1;
		queue_dirty(ss, name0);
	}
}

static void reevaluate_reverse_installif_pkg(struct apk_solver_state *ss, struct apk_package *pkg)
{
	reevaluate_reverse_installif(ss, pkg->name);
	apk_array_foreach(d, pkg->provides)
		reevaluate_reverse_installif(ss, d->name);
}

static void disqualify_package(struct apk_solver_state *ss, struct apk_package *pkg, const char *reason)
{
	dbg_printf("disqualify_package: " PKG_VER_FMT " (%s)\n", PKG_VER_PRINTF(pkg), reason);
	pkg->ss.pkg_selectable = 0;
	reevaluate_reverse_deps(ss, pkg->name);
	apk_array_foreach(p, pkg->provides)
		reevaluate_reverse_deps(ss, p->name);
	reevaluate_reverse_installif_pkg(ss, pkg);
}

static bool dependency_satisfiable(struct apk_solver_state *ss, const struct apk_package *dpkg, struct apk_dependency *dep)
{
	struct apk_name *name = dep->name;

	if (apk_dep_conflict(dep) && ss->ignore_conflict) return true;
	if (name->ss.locked) return apk_dep_is_provided(dpkg, dep, &name->ss.chosen);
	if (name->ss.requirers == 0 && apk_dep_is_provided(dpkg, dep, &provider_none))
		return true;

	apk_array_foreach(p, name->providers)
		if (p->pkg->ss.pkg_selectable && apk_dep_is_provided(dpkg, dep, p))
			return true;

	return false;
}

static void discover_name(struct apk_solver_state *ss, struct apk_name *name)
{
	struct apk_database *db = ss->db;
	unsigned int repos, num_virtual = 0;

	if (name->ss.seen) return;

	name->ss.seen = 1;
	name->ss.no_iif = 1;
	apk_array_foreach(p, name->providers) {
		struct apk_package *pkg = p->pkg;
		if (!pkg->ss.seen) {
			pkg->ss.seen = 1;
			pkg->ss.pinning_allowed = APK_DEFAULT_PINNING_MASK;
			pkg->ss.pinning_preferred = APK_DEFAULT_PINNING_MASK;
			pkg->ss.pkg_available = pkg->filename_ndx || apk_db_pkg_available(db, pkg);
			/* Package is in 'cached' repository if filename is provided,
			 * or it's a 'virtual' package with install_size zero */
			pkg->ss.pkg_selectable = !pkg->uninstallable &&
				(BIT(pkg->layer) & db->active_layers) &&
				(pkg->ss.pkg_available ||
				 pkg->cached || pkg->filename_ndx ||
				 pkg->cached_non_repository ||
				 pkg->installed_size == 0 ||  pkg->ipkg);

			/* Prune install_if packages that are no longer available,
			 * currently works only if SOLVERF_AVAILABLE is set in the
			 * global solver flags. */
			pkg->ss.iif_failed =
				(apk_array_len(pkg->install_if) == 0) ||
				((ss->solver_flags_inherit & APK_SOLVERF_AVAILABLE) &&
				 !pkg->ss.pkg_available);

			repos = get_pkg_repos(db, pkg);
			pkg->ss.tag_preferred = pkg->filename_ndx ||
				(pkg->installed_size == 0) ||
				(repos & ss->default_repos);
			pkg->ss.tag_ok =
				pkg->ss.tag_preferred ||
				pkg->cached_non_repository ||
				pkg->ipkg;

			apk_array_foreach(dep, pkg->depends)
				discover_name(ss, dep->name);

			dbg_printf("discover " PKG_VER_FMT ": tag_ok=%d, tag_pref=%d selectable=%d\n",
				PKG_VER_PRINTF(pkg),
				pkg->ss.tag_ok,
				pkg->ss.tag_preferred,
				pkg->ss.pkg_selectable);
		}

		name->ss.no_iif &= pkg->ss.iif_failed;
		num_virtual += (p->pkg->name != name);
	}

	apk_array_foreach_item(name0, name->rinstall_if)
		discover_name(ss, name0);

	apk_array_foreach(p, name->providers) {
		struct apk_package *pkg = p->pkg;
		apk_array_foreach_item(name0, pkg->name->rinstall_if)
			discover_name(ss, name0);
		apk_array_foreach(dep, pkg->provides)
			discover_name(ss, dep->name);
	}

	unsigned int order_flags = 0;
	if (!name->solver_flags_set) order_flags |= 1UL << 31;
	if (apk_array_len(name->providers) != num_virtual) order_flags |= 1UL << 30;
	name->ss.order_id = order_flags | ++ss->order_id;

	apk_array_foreach(p, name->providers) {
		apk_array_foreach(dep, p->pkg->install_if)
			discover_name(ss, dep->name);
	}

	dbg_printf("discover %s: no_iif=%d num_virtual=%d, order_id=%#x\n",
		name->name, name->ss.no_iif, num_virtual, name->ss.order_id);
}

static void name_requirers_changed(struct apk_solver_state *ss, struct apk_name *name)
{
	queue_unresolved(ss, name, false);
	reevaluate_reverse_installif(ss, name);
	queue_dirty(ss, name);
}

static void inherit_pinning_and_flags(
	struct apk_solver_state *ss, struct apk_package *pkg, struct apk_package *ppkg)
{
	unsigned int repos = get_pkg_repos(ss->db, pkg);

	if (ppkg != NULL) {
		/* inherited */
		pkg->ss.solver_flags |= ppkg->ss.solver_flags_inheritable;
		pkg->ss.solver_flags_inheritable |= ppkg->ss.solver_flags_inheritable;
		pkg->ss.pinning_allowed |= ppkg->ss.pinning_allowed;
	} else {
		/* world dependency */
		pkg->ss.solver_flags |= ss->solver_flags_inherit;
		pkg->ss.solver_flags_inheritable |= ss->solver_flags_inherit;
		pkg->ss.pinning_allowed |= ss->pinning_inherit;
		/* also prefer main pinnings */
		pkg->ss.pinning_preferred = ss->pinning_inherit;
		pkg->ss.tag_preferred = !!(repos & apk_db_get_pinning_mask_repos(ss->db, pkg->ss.pinning_preferred));
	}
	pkg->ss.tag_ok |= !!(repos & apk_db_get_pinning_mask_repos(ss->db, pkg->ss.pinning_allowed));

	dbg_printf(PKG_VER_FMT ": tag_ok=%d, tag_pref=%d\n",
		PKG_VER_PRINTF(pkg), pkg->ss.tag_ok, pkg->ss.tag_preferred);
}

static void apply_constraint(struct apk_solver_state *ss, struct apk_package *ppkg, struct apk_dependency *dep)
{
	struct apk_name *name = dep->name;
	int is_provided;

	dbg_printf("    apply_constraint: %s%s%s" BLOB_FMT "\n",
		apk_dep_conflict(dep) ? "!" : "",
		name->name,
		apk_version_op_string(dep->op),
		BLOB_PRINTF(*dep->version));

	if (apk_dep_conflict(dep) && ss->ignore_conflict)
		return;

	name->ss.requirers += !apk_dep_conflict(dep);
	if (name->ss.requirers == 1 && !apk_dep_conflict(dep))
		name_requirers_changed(ss, name);

	apk_array_foreach(p0, name->providers) {
		struct apk_package *pkg0 = p0->pkg;

		is_provided = apk_dep_is_provided(ppkg, dep, p0);
		dbg_printf("    apply_constraint: provider: %s-" BLOB_FMT ": %d\n",
			pkg0->name->name, BLOB_PRINTF(*p0->version), is_provided);

		pkg0->ss.conflicts += !is_provided;
		if (unlikely(pkg0->ss.pkg_selectable && pkg0->ss.conflicts))
			disqualify_package(ss, pkg0, "conflicting dependency");

		if (is_provided)
			inherit_pinning_and_flags(ss, pkg0, ppkg);
	}
}

static void exclude_non_providers(struct apk_solver_state *ss, struct apk_name *name, struct apk_name *must_provide, int skip_virtuals)
{
	if (name == must_provide || ss->ignore_conflict) return;
	dbg_printf("%s must provide %s (skip_virtuals=%d)\n", name->name, must_provide->name, skip_virtuals);
	apk_array_foreach(p, name->providers) {
		if (p->pkg->name == must_provide || !p->pkg->ss.pkg_selectable ||
		    (skip_virtuals && p->version == &apk_atom_null))
			goto next;
		apk_array_foreach(d, p->pkg->provides)
			if (d->name == must_provide || (skip_virtuals && d->version == &apk_atom_null))
				goto next;
		disqualify_package(ss, p->pkg, "provides transitivity");
	next: ;
	}
}

static inline int merge_index(unsigned short *index, int num_options)
{
	if (*index != num_options) return 0;
	*index = num_options + 1;
	return 1;
}

static inline int merge_index_complete(unsigned short *index, int num_options)
{
	int ret;

	ret = (*index == num_options);
	*index = 0;

	return ret;
}

static bool is_provider_auto_selectable(struct apk_provider *p)
{
	// Virtual packages without provider_priority cannot be autoselected without provider_priority
	if (p->version != &apk_atom_null) return true;
	if (p->pkg->provider_priority) return true;
	if (p->pkg->name->ss.requirers) return true;
	return false;
}

static void reconsider_name(struct apk_solver_state *ss, struct apk_name *name)
{
	struct apk_package *first_candidate = NULL, *pkg;
	int reevaluate_deps, reevaluate_iif;
	int num_options = 0, num_tag_not_ok = 0, has_iif = 0, no_iif = 1;
	bool reevaluate = false;

	dbg_printf("reconsider_name: %s\n", name->name);

	reevaluate_deps = name->ss.reevaluate_deps;
	reevaluate_iif = name->ss.reevaluate_iif;
	name->ss.reevaluate_deps = 0;
	name->ss.reevaluate_iif = 0;

	/* propagate down by merging common dependencies and
	 * applying new constraints */
	unsigned int has_auto_selectable = 0;
	apk_array_foreach(p, name->providers) {
		/* check if this pkg's dependencies have become unsatisfiable */
		pkg = p->pkg;
		pkg->ss.dependencies_merged = 0;
		if (reevaluate_deps) {
			if (!pkg->ss.pkg_selectable)
				continue;
			apk_array_foreach(dep, pkg->depends) {
				if (!dependency_satisfiable(ss, pkg, dep)) {
					disqualify_package(ss, pkg, "dependency no longer satisfiable");
					break;
				}
			}
		}
		if (!pkg->ss.pkg_selectable)
			continue;

		if (reevaluate_iif &&
		    (pkg->ss.iif_triggered == 0 &&
		     pkg->ss.iif_failed == 0)) {
			pkg->ss.iif_triggered = 1;
			pkg->ss.iif_failed = 0;
			apk_array_foreach(dep, pkg->install_if) {
				if (!dep->name->ss.locked) {
					if (apk_dep_conflict(dep)) {
						dep->name->ss.iif_needed = true;
						queue_unresolved(ss, dep->name, false);
					}
					pkg->ss.iif_triggered = 0;
					pkg->ss.iif_failed = 0;
					break;
				}
				if (!apk_dep_is_provided(pkg, dep, &dep->name->ss.chosen)) {
					pkg->ss.iif_triggered = 0;
					pkg->ss.iif_failed = 1;
					break;
				}
			}
		}
		if (reevaluate_iif && pkg->ss.iif_triggered) {
			apk_array_foreach(dep, pkg->install_if)
				inherit_pinning_and_flags(ss, pkg, dep->name->ss.chosen.pkg);
		}
		has_iif |= pkg->ss.iif_triggered;
		no_iif  &= pkg->ss.iif_failed;
		dbg_printf("  "PKG_VER_FMT": iif_triggered=%d iif_failed=%d, no_iif=%d\n",
			PKG_VER_PRINTF(pkg), pkg->ss.iif_triggered, pkg->ss.iif_failed,
			no_iif);
		has_auto_selectable |= pkg->ss.iif_triggered;

		if (name->ss.requirers == 0)
			continue;

		/* merge common dependencies */
		pkg->ss.dependencies_merged = 1;
		if (first_candidate == NULL)
			first_candidate = pkg;

		/* FIXME: can merge also conflicts */
		apk_array_foreach(dep, pkg->depends)
			if (!apk_dep_conflict(dep))
				merge_index(&dep->name->ss.merge_depends, num_options);

		if (merge_index(&pkg->name->ss.merge_provides, num_options))
			pkg->name->ss.has_virtual_provides |= (p->version == &apk_atom_null);
		apk_array_foreach(dep, pkg->provides)
			if (merge_index(&dep->name->ss.merge_provides, num_options))
				dep->name->ss.has_virtual_provides |= (dep->version == &apk_atom_null);

		num_tag_not_ok += !pkg->ss.tag_ok;
		num_options++;
		if (!has_auto_selectable && is_provider_auto_selectable(p))
			has_auto_selectable = 1;
	}
	name->ss.has_options = (num_options > 1 || num_tag_not_ok > 0);
	name->ss.has_iif = has_iif;
	name->ss.no_iif = no_iif;
	if (has_auto_selectable != name->ss.has_auto_selectable) {
		name->ss.has_auto_selectable = has_auto_selectable;
		reevaluate = true;
	}

	if (first_candidate != NULL) {
		pkg = first_candidate;
		apk_array_foreach(p, name->providers)
			p->pkg->ss.dependencies_used = p->pkg->ss.dependencies_merged;

		/* propagate down common dependencies */
		if (num_options == 1) {
			/* FIXME: keeps increasing counts, use bit fields instead? */
			apk_array_foreach(dep, pkg->depends)
				if (merge_index_complete(&dep->name->ss.merge_depends, num_options))
					apply_constraint(ss, pkg, dep);
		} else {
			/* FIXME: could merge versioning bits too */
			apk_array_foreach(dep, pkg->depends) {
				struct apk_name *name0 = dep->name;
				if (merge_index_complete(&name0->ss.merge_depends, num_options) &&
				    name0->ss.requirers == 0) {
					/* common dependency name with all */
					dbg_printf("%s common dependency: %s\n",
						   name->name, name0->name);
					name0->ss.requirers++;
					name_requirers_changed(ss, name0);
					apk_array_foreach(p, name0->providers)
						inherit_pinning_and_flags(ss, p->pkg, pkg);
				}
			}
		}

		/* provides transitivity */
		if (merge_index_complete(&pkg->name->ss.merge_provides, num_options))
			exclude_non_providers(ss, pkg->name, name, pkg->name->ss.has_virtual_provides);
		apk_array_foreach(dep, pkg->provides)
			if (merge_index_complete(&dep->name->ss.merge_provides, num_options))
				exclude_non_providers(ss, dep->name, name, dep->name->ss.has_virtual_provides);

		pkg->name->ss.has_virtual_provides = 0;
		apk_array_foreach(dep, pkg->provides)
			dep->name->ss.has_virtual_provides = 0;
	}

	name->ss.reverse_deps_done = 1;
	apk_array_foreach_item(name0, name->rdepends) {
		if (name0->ss.seen && !name0->ss.locked) {
			name->ss.reverse_deps_done = 0;
			break;
		}
	}
	queue_unresolved(ss, name, reevaluate);

	dbg_printf("reconsider_name: %s [finished], has_options=%d, has_autoselectable=%d, reverse_deps_done=%d\n",
		name->name, name->ss.has_options, name->ss.has_auto_selectable, name->ss.reverse_deps_done);
}

static int compare_providers(struct apk_solver_state *ss,
			     struct apk_provider *pA, struct apk_provider *pB)
{
	struct apk_database *db = ss->db;
	struct apk_package *pkgA = pA->pkg, *pkgB = pB->pkg;
	unsigned int solver_flags;
	int r;

	/* Prefer existing package */
	if (pkgA == NULL || pkgB == NULL) {
		dbg_printf("   prefer existing package\n");
		return (pkgA != NULL) - (pkgB != NULL);
	}
	solver_flags = pkgA->ss.solver_flags | pkgB->ss.solver_flags;

	/* Latest version required? */
	if ((solver_flags & APK_SOLVERF_LATEST) &&
	    (pkgA->ss.pinning_allowed == APK_DEFAULT_PINNING_MASK) &&
	    (pkgB->ss.pinning_allowed == APK_DEFAULT_PINNING_MASK)) {
		/* Prefer allowed pinning */
		r = (int)pkgA->ss.tag_ok - (int)pkgB->ss.tag_ok;
		if (r) {
			dbg_printf("    prefer allowed pinning\n");
			return r;
		}

		/* Prefer available */
		if (solver_flags & APK_SOLVERF_AVAILABLE) {
			r = (int)pkgA->ss.pkg_available - (int)pkgB->ss.pkg_available;
			if (r) {
				dbg_printf("    prefer available\n");
				return r;
			}
		} else if (solver_flags & APK_SOLVERF_REINSTALL) {
			r = (int)pkgA->ss.pkg_selectable - (int)pkgB->ss.pkg_selectable;
			if (r) {
				dbg_printf("    prefer available (reinstall)\n");
				return r;
			}
		}
	} else {
		/* Prefer without errors */
		r = (int)pkgA->ss.pkg_selectable - (int)pkgB->ss.pkg_selectable;
		if (r) {
			dbg_printf("    prefer without errors\n");
			return r;
		}

		/* Prefer those that were in last dependency merging group */
		r = (int)pkgA->ss.dependencies_used - (int)pkgB->ss.dependencies_used;
		if (r) {
			dbg_printf("    prefer those that were in last dependency merging group\n");
			return r;
		}
		r = pkgB->ss.conflicts - pkgA->ss.conflicts;
		if (r) {
			dbg_printf("    prefer those that were in last dependency merging group (#2)\n");
			return r;
		}

		/* Prefer installed on self-upgrade */
		if ((db->performing_preupgrade && !(solver_flags & APK_SOLVERF_UPGRADE)) ||
		    (solver_flags & APK_SOLVERF_INSTALLED)) {
			r = (pkgA->ipkg != NULL) - (pkgB->ipkg != NULL);
			if (r) {
				dbg_printf("    prefer installed (preupgrade)\n");
				return r;
			}
		}

		/* Prefer allowed pinning */
		r = (int)pkgA->ss.tag_ok - (int)pkgB->ss.tag_ok;
		if (r) {
			dbg_printf("    prefer allowed pinning\n");
			return r;
		}

		/* Prefer available */
		if (solver_flags & APK_SOLVERF_AVAILABLE) {
			r = (int)pkgA->ss.pkg_available - (int)pkgB->ss.pkg_available;
			if (r) {
				dbg_printf("    prefer available\n");
				return r;
			}
		}

		/* Prefer preferred pinning */
		r = (int)pkgA->ss.tag_preferred - (int)pkgB->ss.tag_preferred;
		if (r) {
			dbg_printf("    prefer preferred pinning\n");
			return r;
		}

		/* Prefer installed */
		if (!(solver_flags & (APK_SOLVERF_REMOVE|APK_SOLVERF_UPGRADE)) &&
		    (pkgA->name == pkgB->name || pA->version != &apk_atom_null || pB->version != &apk_atom_null)) {
			r = (pkgA->ipkg != NULL) - (pkgB->ipkg != NULL);
			if (r) {
				dbg_printf("    prefer installed (non-upgrade)\n");
				return r;
			}
		}
	}

	/* Select latest by requested name */
	switch (apk_version_compare(*pA->version, *pB->version)) {
	case APK_VERSION_LESS:
		dbg_printf("    select latest by requested name (less)\n");
		return -1;
	case APK_VERSION_GREATER:
		dbg_printf("    select latest by requested name (greater)\n");
		return 1;
	}

	/* Select latest by principal name */
	if (pkgA->name == pkgB->name) {
		switch (apk_version_compare(*pkgA->version, *pkgB->version)) {
		case APK_VERSION_LESS:
			dbg_printf("    select latest by principal name (less)\n");
			return -1;
		case APK_VERSION_GREATER:
			dbg_printf("    select latest by principal name (greater)\n");
			return 1;
		}
	}

	/* Prefer highest declared provider priority. */
	r = pkgA->provider_priority - pkgB->provider_priority;
	if (r) {
		dbg_printf("    prefer highest declared provider priority\n");
		return r;
	}

	/* Prefer installed (matches here if upgrading) */
	if (!(solver_flags & APK_SOLVERF_REMOVE)) {
		r = (pkgA->ipkg != NULL) - (pkgB->ipkg != NULL);
		if (r) {
			dbg_printf("    prefer installed (upgrading)\n");
			return r;
		}
	}

	/* Prefer without errors (mostly if --latest used, and different provider) */
	r = (int)pkgA->ss.pkg_selectable - (int)pkgB->ss.pkg_selectable;
	if (r) {
		dbg_printf("    prefer without errors (#2)\n");
		return r;
	}

	/* Prefer lowest available repository */
	dbg_printf("    prefer lowest available repository\n");
	return ffs(pkgB->repos) - ffs(pkgA->repos);
}

static void assign_name(struct apk_solver_state *ss, struct apk_name *name, struct apk_provider p)
{
	if (name->ss.locked) {
		/* If both are providing this name without version, it's ok */
		if (p.version == &apk_atom_null &&
		    name->ss.chosen.version == &apk_atom_null)
			return;
		if (ss->ignore_conflict)
			return;
		/* Conflict: providing same name */
		mark_error(ss, p.pkg, "conflict: same name provided");
		mark_error(ss, name->ss.chosen.pkg, "conflict: same name provided");
		return;
	}

	if (p.pkg) dbg_printf("assign %s to "PKG_VER_FMT"\n", name->name, PKG_VER_PRINTF(p.pkg));
	else dbg_printf("assign %s to <none>\n", name->name);

	name->ss.locked = 1;
	name->ss.chosen = p;
	if (list_hashed(&name->ss.unresolved_list))
		list_del(&name->ss.unresolved_list);
	if (list_hashed(&name->ss.dirty_list))
		list_del(&name->ss.dirty_list);

	if (p.pkg && !name->ss.requirers && p.pkg->ss.iif_triggered) {
		apk_array_foreach(dep, p.pkg->install_if)
			if (!dep->name->ss.locked) apply_constraint(ss, p.pkg, dep);
	}

	/* disqualify all conflicting packages */
	if (!ss->ignore_conflict) {
		apk_array_foreach(p0, name->providers) {
			if (p0->pkg == p.pkg) continue;
			if (p.version == &apk_atom_null &&
			    p0->version == &apk_atom_null)
				continue;
			disqualify_package(ss, p0->pkg, "conflicting provides");
		}
	}
	reevaluate_reverse_deps(ss, name);
	if (p.pkg)
		reevaluate_reverse_installif_pkg(ss, p.pkg);
	else
		reevaluate_reverse_installif(ss, name);
}

static void select_package(struct apk_solver_state *ss, struct apk_name *name)
{
	struct apk_provider chosen = { NULL, &apk_atom_null };
	struct apk_package *pkg = NULL;

	dbg_printf("select_package: %s (requirers=%d, autosel=%d, iif=%d, order_id=%#x)\n",
		name->name, name->ss.requirers, name->ss.has_auto_selectable, name->ss.has_iif, name->ss.order_id);

	if (name->ss.requirers || name->ss.has_iif) {
		apk_array_foreach(p, name->providers) {
			dbg_printf("  consider "PKG_VER_FMT" iif_triggered=%d, tag_ok=%d, selectable=%d, available=%d, flags=0x%x, provider_priority=%d, installed=%d\n",
				PKG_VER_PRINTF(p->pkg),
				p->pkg->ss.iif_triggered, p->pkg->ss.tag_ok,
				p->pkg->ss.pkg_selectable, p->pkg->ss.pkg_available,
				p->pkg->ss.solver_flags,
				p->pkg->provider_priority, p->pkg->ipkg != NULL);
			/* Ensure valid pinning and install-if trigger */
			if (name->ss.requirers == 0 &&
			    (!p->pkg->ss.iif_triggered ||
			     !p->pkg->ss.tag_ok ||
			     !p->pkg->ss.pkg_selectable)) {
				dbg_printf("    ignore: invalid install-if trigger or invalid pinning\n");
				continue;
			}
			if (!is_provider_auto_selectable(p)) {
				dbg_printf("    ignore: virtual package without provider_priority\n");
				continue;
			}
			if (compare_providers(ss, p, &chosen) > 0) {
				dbg_printf("    choose as new provider\n");
				chosen = *p;
			}
		}
	}

	pkg = chosen.pkg;
	if (pkg) {
		if (!pkg->ss.pkg_selectable || !pkg->ss.tag_ok) {
			/* Selecting broken or unallowed package */
			mark_error(ss, pkg, "broken package / tag not ok");
		}
		dbg_printf("selecting: " PKG_VER_FMT ", available: %d\n", PKG_VER_PRINTF(pkg), pkg->ss.pkg_selectable);

		assign_name(ss, pkg->name, APK_PROVIDER_FROM_PACKAGE(pkg));
		apk_array_foreach(d, pkg->provides)
			assign_name(ss, d->name, APK_PROVIDER_FROM_PROVIDES(pkg, d));

		apk_array_foreach(d, pkg->depends)
			apply_constraint(ss, pkg, d);
	} else {
		dbg_printf("selecting: %s [unassigned]\n", name->name);
		assign_name(ss, name, provider_none);
		if (name->ss.requirers > 0) {
			dbg_printf("ERROR NO-PROVIDER: %s\n", name->name);
			ss->errors++;
		}
	}
}

static void record_change(struct apk_solver_state *ss, struct apk_package *opkg, struct apk_package *npkg)
{
	struct apk_changeset *changeset = ss->changeset;
	struct apk_change *change;

	change = apk_change_array_add(&changeset->changes, (struct apk_change) {
		.old_pkg = opkg,
		.old_repository_tag = opkg ? opkg->ipkg->repository_tag : 0,
		.new_pkg = npkg,
		.new_repository_tag = npkg ? get_tag(ss->db, npkg->ss.pinning_allowed, get_pkg_repos(ss->db, npkg)) : 0,
		.reinstall = npkg ? !!(npkg->ss.solver_flags & APK_SOLVERF_REINSTALL) : 0,
	});
	if (npkg == NULL)
		changeset->num_remove++;
	else if (opkg == NULL)
		changeset->num_install++;
	else if (npkg != opkg || change->reinstall || change->new_repository_tag != change->old_repository_tag)
		changeset->num_adjust++;
}

static void cset_gen_name_change(struct apk_solver_state *ss, struct apk_name *name);
static void cset_gen_name_remove(struct apk_solver_state *ss, struct apk_package *pkg);
static void cset_gen_dep(struct apk_solver_state *ss, struct apk_package *ppkg, struct apk_dependency *dep);

static void cset_track_deps_added(struct apk_package *pkg)
{
	apk_array_foreach(d, pkg->depends) {
		if (apk_dep_conflict(d) || !d->name->ss.installed_name) continue;
		d->name->ss.installed_name->ss.requirers++;
	}
}

static void cset_track_deps_removed(struct apk_solver_state *ss, struct apk_package *pkg)
{
	struct apk_package *pkg0;

	apk_array_foreach(d, pkg->depends) {
		if (apk_dep_conflict(d) || !d->name->ss.installed_name)
			continue;
		if (--d->name->ss.installed_name->ss.requirers > 0)
			continue;
		pkg0 = d->name->ss.installed_pkg;
		if (pkg0 != NULL)
			cset_gen_name_remove(ss, pkg0);
	}
}

static void cset_check_removal_by_deps(struct apk_solver_state *ss, struct apk_package *pkg)
{
	/* NOTE: an orphaned package name may have 0 requirers because it is now being satisfied
	 * through an alternate provider.  In these cases, we will handle this later as an adjustment
	 * operation using cset_gen_name_change().  As such, only insert a removal into the transaction
	 * if there is no other resolved provider.
	 */
	if (pkg->name->ss.requirers == 0 && pkg->name->ss.chosen.pkg == NULL)
		cset_gen_name_remove(ss, pkg);
}

static void cset_check_install_by_iif(struct apk_solver_state *ss, struct apk_name *name)
{
	struct apk_package *pkg = name->ss.chosen.pkg;

	if (!pkg || !name->ss.seen || name->ss.changeset_processed) return;

	apk_array_foreach(dep0, pkg->install_if) {
		struct apk_name *name0 = dep0->name;
		if (!apk_dep_conflict(dep0) && !name0->ss.changeset_processed) return;
		if (!apk_dep_is_provided(pkg, dep0, &name0->ss.chosen)) return;
	}
	cset_gen_name_change(ss, name);
}

static void cset_check_removal_by_iif(struct apk_solver_state *ss, struct apk_name *name)
{
	struct apk_package *pkg = name->ss.installed_pkg;

	if (!pkg || name->ss.chosen.pkg) return;
	if (name->ss.changeset_processed || name->ss.changeset_removed) return;

	apk_array_foreach(dep0, pkg->install_if) {
		struct apk_name *name0 = dep0->name;
		if (name0->ss.changeset_removed && !name0->ss.chosen.pkg) {
			cset_check_removal_by_deps(ss, pkg);
			return;
		}
	}
}

static void cset_check_by_reverse_iif(struct apk_solver_state *ss, struct apk_package *pkg, void (*cb)(struct apk_solver_state *ss, struct apk_name *))
{
	if (!pkg) return;
	apk_array_foreach_item(name, pkg->name->rinstall_if) cb(ss, name);
	apk_array_foreach(d, pkg->provides)
		apk_array_foreach_item(name, d->name->rinstall_if) cb(ss, name);
}

static void cset_gen_name_preprocess(struct apk_solver_state *ss, struct apk_name *name)
{
	if (name->ss.changeset_processed) return;
	name->ss.changeset_processed = 1;

	dbg_printf("cset_gen_name_remove_orphans: %s\n", name->name);

	/* Remove the package providing this name previously if it was provided
	 * by a package with different name. */
	if (name->ss.installed_pkg && (!name->ss.chosen.pkg || name->ss.chosen.pkg->name != name))
		cset_gen_name_remove(ss, name->ss.installed_pkg);

	/* Remove any package that provides this name and is due to be deleted */
	apk_array_foreach(p, name->providers) {
		struct apk_package *pkg0 = p->pkg;
		struct apk_name *name0 = pkg0->name;
		if (name0->ss.installed_pkg == pkg0 && name0->ss.chosen.pkg == NULL)
			cset_gen_name_remove(ss, pkg0);
	}
}

static void cset_gen_name_change(struct apk_solver_state *ss, struct apk_name *name)
{
	struct apk_package *pkg, *opkg;

	if (name->ss.changeset_processed) return;

	dbg_printf("cset_gen: processing: %s\n", name->name);
	cset_gen_name_preprocess(ss, name);

	pkg = name->ss.chosen.pkg;
	if (!pkg || pkg->ss.in_changeset) return;

	pkg->ss.in_changeset = 1;
	cset_gen_name_preprocess(ss, pkg->name);
	apk_array_foreach(d, pkg->provides)
		cset_gen_name_preprocess(ss, d->name);

	opkg = pkg->name->ss.installed_pkg;
	cset_check_by_reverse_iif(ss, opkg, cset_check_removal_by_iif);

	apk_array_foreach(d, pkg->depends)
		cset_gen_dep(ss, pkg, d);

	dbg_printf("cset_gen: selecting: "PKG_VER_FMT"%s\n", PKG_VER_PRINTF(pkg), pkg->ss.pkg_selectable ? "" : " [NOT SELECTABLE]");
	record_change(ss, opkg, pkg);

	cset_check_by_reverse_iif(ss, pkg, cset_check_install_by_iif);

	cset_track_deps_added(pkg);
	if (opkg)
		cset_track_deps_removed(ss, opkg);
}

static void cset_gen_name_remove0(struct apk_package *pkg0, struct apk_dependency *dep0, struct apk_package *pkg, void *ctx)
{
	cset_gen_name_remove(ctx, pkg0);
}

static void cset_gen_name_remove(struct apk_solver_state *ss, struct apk_package *pkg)
{
	struct apk_name *name = pkg->name;

	if (pkg->ss.in_changeset ||
	    (name->ss.chosen.pkg != NULL &&
	     name->ss.chosen.pkg->name == name))
		return;

	name->ss.changeset_removed = 1;
	pkg->ss.in_changeset = 1;
	apk_pkg_foreach_reverse_dependency(pkg, APK_FOREACH_INSTALLED|APK_DEP_SATISFIES, cset_gen_name_remove0, ss);
	cset_check_by_reverse_iif(ss, pkg, cset_check_removal_by_iif);

	record_change(ss, pkg, NULL);
	cset_track_deps_removed(ss, pkg);
}

static void cset_gen_dep(struct apk_solver_state *ss, struct apk_package *ppkg, struct apk_dependency *dep)
{
	struct apk_name *name = dep->name;
	struct apk_package *pkg = name->ss.chosen.pkg;

	if (apk_dep_conflict(dep) && ss->ignore_conflict)
		return;

	if (!apk_dep_is_provided(ppkg, dep, &name->ss.chosen))
		mark_error(ss, ppkg, "unfulfilled dependency");

	cset_gen_name_change(ss, name);

	if (pkg && pkg->ss.error)
		mark_error(ss, ppkg, "propagation up");
}

static int cset_reset_name(apk_hash_item item, void *ctx)
{
	struct apk_name *name = (struct apk_name *) item;
	name->ss.installed_pkg = NULL;
	name->ss.installed_name = NULL;
	name->ss.requirers = 0;
	return 0;
}

static void generate_changeset(struct apk_solver_state *ss, struct apk_dependency_array *world)
{
	struct apk_changeset *changeset = ss->changeset;
	struct apk_package *pkg;
	struct apk_installed_package *ipkg;

	apk_array_truncate(changeset->changes, 0);

	apk_hash_foreach(&ss->db->available.names, cset_reset_name, NULL);
	list_for_each_entry(ipkg, &ss->db->installed.packages, installed_pkgs_list) {
		pkg = ipkg->pkg;
		pkg->name->ss.installed_pkg = pkg;
		pkg->name->ss.installed_name = pkg->name;
		apk_array_foreach(d, pkg->provides)
			if (d->version != &apk_atom_null)
				d->name->ss.installed_name = pkg->name;
	}
	list_for_each_entry(ipkg, &ss->db->installed.packages, installed_pkgs_list)
		cset_track_deps_added(ipkg->pkg);
	list_for_each_entry(ipkg, &ss->db->installed.packages, installed_pkgs_list)
		cset_check_removal_by_deps(ss, ipkg->pkg);

	apk_array_foreach(d, world)
		cset_gen_dep(ss, NULL, d);

	/* NOTE: We used to call cset_gen_name_remove() directly here.  While slightly faster, this clobbered
	 * dependency nodes where a new package was provided under a different name (using provides).  As such,
	 * treat everything as a change first and then call cset_gen_name_remove() from there if appropriate.
	 */
	list_for_each_entry(ipkg, &ss->db->installed.packages, installed_pkgs_list)
		cset_gen_name_change(ss, ipkg->pkg->name);

	changeset->num_total_changes =
		changeset->num_install +
		changeset->num_remove +
		changeset->num_adjust;
}

static int free_name(apk_hash_item item, void *ctx)
{
	struct apk_name *name = (struct apk_name *) item;
	memset(&name->ss, 0, sizeof(name->ss));
	return 0;
}

static int free_package(apk_hash_item item, void *ctx)
{
	struct apk_package *pkg = (struct apk_package *) item;
	memset(&pkg->ss, 0, sizeof(pkg->ss));
	return 0;
}

static int cmp_pkgname(const void *p1, const void *p2)
{
	const struct apk_dependency *d1 = p1, *d2 = p2;
	return apk_name_cmp_display(d1->name, d2->name);
}

static struct apk_name *dequeue_next_name(struct apk_solver_state *ss)
{
	if (!list_empty(&ss->resolvenow_head)) {
		struct apk_name *name = list_pop(&ss->resolvenow_head, struct apk_name, ss.unresolved_list);
		dbg_printf("name <%s> selected from resolvenow list\n", name->name);
		return name;
	}
	if (!list_empty(&ss->selectable_head)) {
		struct apk_name *name = list_pop(&ss->selectable_head, struct apk_name, ss.unresolved_list);
		dbg_printf("name <%s> selected from selectable list\n", name->name);
		return name;
	}
	if (!list_empty(&ss->unresolved_head)) {
		struct apk_name *name = list_pop(&ss->unresolved_head, struct apk_name, ss.unresolved_list);
		dbg_printf("name <%s> selected from unresolved list\n", name->name);
		return name;
	}
	return NULL;
}

int apk_solver_solve(struct apk_database *db,
		     unsigned short solver_flags,
		     struct apk_dependency_array *world,
		     struct apk_changeset *changeset)
{
	struct apk_name *name;
	struct apk_package *pkg;
	struct apk_solver_state ss_data, *ss = &ss_data;

	apk_array_qsort(world, cmp_pkgname);

restart:
	memset(ss, 0, sizeof(*ss));
	ss->db = db;
	ss->changeset = changeset;
	ss->default_repos = apk_db_get_pinning_mask_repos(db, APK_DEFAULT_PINNING_MASK);
	ss->ignore_conflict = !!(solver_flags & APK_SOLVERF_IGNORE_CONFLICT);
	list_init(&ss->dirty_head);
	list_init(&ss->unresolved_head);
	list_init(&ss->selectable_head);
	list_init(&ss->resolvenow_head);

	dbg_printf("discovering world\n");
	ss->solver_flags_inherit = solver_flags;
	apk_array_foreach(d, world) {
		if (!d->broken)
			discover_name(ss, d->name);
	}
	dbg_printf("applying world\n");
	apk_array_foreach(d, world) {
		if (!d->broken) {
			ss->pinning_inherit = BIT(d->repository_tag);
			apply_constraint(ss, NULL, d);
		}
	}
	ss->solver_flags_inherit = 0;
	ss->pinning_inherit = 0;
	dbg_printf("applying world [finished]\n");

	do {
		while (!list_empty(&ss->dirty_head)) {
			name = list_pop(&ss->dirty_head, struct apk_name, ss.dirty_list);
			reconsider_name(ss, name);
		}
		name = dequeue_next_name(ss);
		if (name == NULL)
			break;
		select_package(ss, name);
	} while (1);

	generate_changeset(ss, world);

	if (ss->errors && (db->ctx->force & APK_FORCE_BROKEN_WORLD)) {
		apk_array_foreach(d, world) {
			name = d->name;
			pkg = name->ss.chosen.pkg;
			if (pkg == NULL || pkg->ss.error) {
				d->broken = 1;
				dbg_printf("disabling broken world dep: %s\n", name->name);
			}
		}
		apk_hash_foreach(&db->available.names, free_name, NULL);
		apk_hash_foreach(&db->available.packages, free_package, NULL);
		goto restart;
	}

	apk_array_foreach(d, world) {
		if (!d->name->ss.chosen.pkg) continue;
		d->layer = d->name->ss.chosen.pkg->layer;
	}

	apk_hash_foreach(&db->available.names, free_name, NULL);
	apk_hash_foreach(&db->available.packages, free_package, NULL);
	dbg_printf("solver done, errors=%d\n", ss->errors);

	return ss->errors;
}
