/* apk_tar.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include "apk_io.h"

int apk_tar_parse(struct apk_istream *,
		  apk_archive_entry_parser parser, void *ctx,
		  struct apk_id_cache *);
int apk_tar_write_entry(struct apk_ostream *, const struct apk_file_info *ae,
			const char *data);
int apk_tar_write_padding(struct apk_ostream *, int size);
