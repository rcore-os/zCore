/* app_policy.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2013 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <stdio.h>
#include "apk_defines.h"
#include "apk_applet.h"
#include "apk_database.h"
#include "apk_version.h"
#include "apk_print.h"

static int policy_main(void *ctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_package_array *pkgs;
	struct apk_name *name = NULL;
	struct apk_out *out = &ac->out;
	struct apk_database *db = ac->db;
	int r;

	ac->query.filter.all_matches = 1;

	apk_package_array_init(&pkgs);
	r = apk_query_packages(ac, &ac->query, args, &pkgs);
	if (r < 0) {
		apk_err(out, "query failed: %s", apk_error_str(r));
		goto err;
	}

	apk_array_foreach_item(pkg, pkgs) {
		/*
		zlib1g policy:
		  2.0:
		    @testing http://nl.alpinelinux.org/alpine/edge/testing
		  1.7:
		    @edge http://nl.alpinelinux.org/alpine/edge/main
		  1.2.3.5 (upgradeable):
		    http://nl.alpinelinux.org/alpine/v2.6/main
		  1.2.3.4 (installed):
		    /media/cdrom/...
		    http://nl.alpinelinux.org/alpine/v2.5/main
		  1.1:
		    http://nl.alpinelinux.org/alpine/v2.4/main
		*/
		if (pkg->name != name) {
			name = pkg->name;
			apk_out(out, "%s policy:", name->name);
		}
		apk_out(out, "  " BLOB_FMT ":", BLOB_PRINTF(*pkg->version));
		if (pkg->ipkg) apk_out(out, "    %s/installed", apk_db_layer_name(pkg->layer));
		for (int i = 0; i < db->num_repos; i++) {
			if (!(BIT(i) & pkg->repos)) continue;
			for (int j = 0; j < db->num_repo_tags; j++) {
				if (db->repo_tags[j].allowed_repos & pkg->repos)
					apk_out(out, "    " BLOB_FMT "%s" BLOB_FMT,
						BLOB_PRINTF(db->repo_tags[j].tag),
						j == 0 ? "" : " ",
						BLOB_PRINTF(db->repos[i].url_printable));
			}
		}
	}
	r = 0;
err:
	apk_package_array_free(&pkgs);
	return r;
}

static struct apk_applet apk_policy = {
	.name = "policy",
	.optgroup_query = 1,
	.open_flags = APK_OPENF_READ | APK_OPENF_ALLOW_ARCH,
	.main = policy_main,
};

APK_DEFINE_APPLET(apk_policy);
