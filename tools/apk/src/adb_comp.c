/* adb_comp.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2021 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include "apk_defines.h"
#include "adb.h"

struct compression_info {
	const char *name;
	uint8_t min_level, max_level;
	struct apk_ostream *(*compress)(struct apk_ostream *, uint8_t);
	struct apk_istream *(*decompress)(struct apk_istream *);
};

static const struct compression_info compression_infos[] = {
	[ADB_COMP_NONE] = {
		.name = "none",
	},
	[ADB_COMP_DEFLATE] = {
		.name = "deflate",
		.compress = apk_ostream_deflate,
		.decompress = apk_istream_deflate,
		.min_level = 0, .max_level = 9,
	},
#ifdef HAVE_ZSTD
	[ADB_COMP_ZSTD] = {
		.name = "zstd",
		.compress = apk_ostream_zstd,
		.decompress = apk_istream_zstd,
		.min_level = 0, .max_level = 22,
	},
#endif
};

static const struct compression_info *compression_info_by_name(const char *name, size_t n, uint8_t *compalg)
{
	for (int i = 0; i < ARRAY_SIZE(compression_infos); i++) {
		const struct compression_info *ci = &compression_infos[i];
		if (strlen(ci->name) == n && strncmp(name, ci->name, n) == 0) {
			*compalg = i;
			return ci;
		}
	}
	return NULL;
}

static const struct compression_info *compression_info_by_alg(uint8_t alg)
{
	if (alg >= ARRAY_SIZE(compression_infos)) return NULL;
	return &compression_infos[alg];
}

int adb_parse_compression(const char *spec_string, struct adb_compression_spec *spec)
{
	const struct compression_info *ci;
	const char *delim = strchrnul(spec_string, ':');
	char *end;
	long level = 0;

	ci = compression_info_by_name(spec_string, delim - spec_string, &spec->alg);
	if (!ci) goto err;
	if (*delim != 0) {
		if (delim[1] == 0) goto err;
		if (ci->max_level == 0) goto err;

		level = strtol(delim+1, &end, 0);
		if (*end != 0) goto err;
		if (level < ci->min_level || level > ci->max_level) goto err;
	}
	if (spec->alg == ADB_COMP_NONE) level = 1;
	spec->level = level;
	return 0;
err:
	*spec = (struct adb_compression_spec) { .alg = ADB_COMP_NONE };
	return -APKE_ADB_COMPRESSION;
}

struct apk_istream *adb_decompress(struct apk_istream *is, struct adb_compression_spec *retspec)
{
	struct adb_compression_spec spec = { .alg = ADB_COMP_NONE };

	if (IS_ERR(is)) return is;

	uint8_t *buf = apk_istream_peek(is, 4);
	if (IS_ERR(buf)) return ERR_PTR(apk_istream_close_error(is, PTR_ERR(buf)));
	if (memcmp(buf, "ADB", 3) != 0) return ERR_PTR(apk_istream_close_error(is, -APKE_ADB_HEADER));
	switch (buf[3]) {
	case '.':
		spec.alg = ADB_COMP_NONE;
		spec.level = 1;
		break;
	case 'd':
		apk_istream_get(is, 4);
		spec.alg = ADB_COMP_DEFLATE;
		break;
	case 'c':
		apk_istream_get(is, 4);
		apk_istream_read(is, &spec, sizeof spec);
		break;
	default:
		goto err;
	}

	const struct compression_info *ci = compression_info_by_alg(spec.alg);
	if (!ci) goto err;

	if (spec.alg != ADB_COMP_NONE)
		is = ci->decompress(is);

	if (retspec) *retspec = spec;

	return is;
err:
	return ERR_PTR(apk_istream_close_error(is, -APKE_ADB_COMPRESSION));
}

struct apk_ostream *adb_compress(struct apk_ostream *os, struct adb_compression_spec *spec)
{
	const struct compression_info *ci;

	if (IS_ERR(os)) return os;
	if (spec->alg == ADB_COMP_NONE && spec->level == 0) {
		*spec = (struct adb_compression_spec) {
			.alg = ADB_COMP_DEFLATE,
		};
	}

	switch (spec->alg) {
	case ADB_COMP_NONE:
		return os;
	case ADB_COMP_DEFLATE:
		if (spec->level != 0) break;
		if (apk_ostream_write(os, "ADBd", 4) < 0) goto err;
		return apk_ostream_deflate(os, 0);
	}

	ci = compression_info_by_alg(spec->alg);
	if (!ci) goto err;
	if (spec->level < ci->min_level || spec->level > ci->max_level) goto err;

	if (apk_ostream_write(os, "ADBc", 4) < 0) goto err;
	if (apk_ostream_write(os, spec, sizeof *spec) < 0) goto err;
	return ci->compress(os, spec->level);

err:
	apk_ostream_cancel(os, -APKE_ADB_COMPRESSION);
	return ERR_PTR(apk_ostream_close(os));
}
