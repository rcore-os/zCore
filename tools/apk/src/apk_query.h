/* apk_query.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2025 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include "apk_defines.h"

struct apk_query_spec;
struct apk_ostream;
struct apk_serializer;
struct apk_string_array;
struct apk_package_array;
struct apk_ctx;
struct apk_database;

enum {
	APK_Q_FIELD_QUERY = 0,
	APK_Q_FIELD_ERROR,

	// who-owns
	APK_Q_FIELD_PATH_TARGET,
	APK_Q_FIELD_OWNER,

	// package fields
	APK_Q_FIELD_PACKAGE,
	APK_Q_FIELD_NAME,
	APK_Q_FIELD_VERSION,
	APK_Q_FIELD_HASH,
	APK_Q_FIELD_DESCRIPTION,
	APK_Q_FIELD_ARCH,
	APK_Q_FIELD_LICENSE,
	APK_Q_FIELD_ORIGIN,
	APK_Q_FIELD_MAINTAINER,
	APK_Q_FIELD_URL,
	APK_Q_FIELD_COMMIT,
	APK_Q_FIELD_BUILD_TIME,
	APK_Q_FIELD_INSTALLED_SIZE,
	APK_Q_FIELD_FILE_SIZE,
	APK_Q_FIELD_PROVIDER_PRIORITY,
	APK_Q_FIELD_DEPENDS,
	APK_Q_FIELD_PROVIDES,
	APK_Q_FIELD_REPLACES,
	APK_Q_FIELD_INSTALL_IF,
	APK_Q_FIELD_RECOMMENDS,
	APK_Q_FIELD_LAYER,
	APK_Q_FIELD_TAGS,

	// installed package fields
	APK_Q_FIELD_CONTENTS,
	APK_Q_FIELD_TRIGGERS,
	APK_Q_FIELD_SCRIPTS,
	APK_Q_FIELD_REPLACES_PRIORITY,

	// installed database fields (for installed packages)
	APK_Q_FIELD_STATUS,

	// repositories fields
	APK_Q_FIELD_REPOSITORIES,
	APK_Q_FIELD_DOWNLOAD_URL,

	// synthetic fields
	APK_Q_FIELD_REV_DEPENDS,
	APK_Q_FIELD_REV_INSTALL_IF,
	APK_Q_NUM_FIELDS
};

#define APK_Q_FIELDS_ALL 		(BIT(APK_Q_NUM_FIELDS)-1)
#define APK_Q_FIELDS_MATCHABLE \
	(BIT(APK_Q_FIELD_PACKAGE) | BIT(APK_Q_FIELD_NAME) | BIT(APK_Q_FIELD_VERSION) | \
	 BIT(APK_Q_FIELD_DESCRIPTION) | BIT(APK_Q_FIELD_ARCH) |BIT(APK_Q_FIELD_LICENSE) | \
	 BIT(APK_Q_FIELD_ORIGIN) | BIT(APK_Q_FIELD_MAINTAINER) | BIT(APK_Q_FIELD_URL) | \
	 BIT(APK_Q_FIELD_PROVIDES) | BIT(APK_Q_FIELD_DEPENDS) | BIT(APK_Q_FIELD_INSTALL_IF) | \
	 BIT(APK_Q_FIELD_RECOMMENDS) | BIT(APK_Q_FIELD_REPLACES) | BIT(APK_Q_FIELD_TAGS) | \
	 BIT(APK_Q_FIELD_CONTENTS) | BIT(APK_Q_FIELD_OWNER))
#define APK_Q_FIELDS_DEFAULT_QUERY	(BIT(APK_Q_FIELD_QUERY) | BIT(APK_Q_FIELD_ERROR))
#define APK_Q_FIELDS_DEFAULT_PKG \
	(APK_Q_FIELDS_DEFAULT_QUERY | BIT(APK_Q_FIELD_NAME) | BIT(APK_Q_FIELD_VERSION) | \
	BIT(APK_Q_FIELD_DESCRIPTION) | BIT(APK_Q_FIELD_ARCH) | BIT(APK_Q_FIELD_LICENSE) | \
	BIT(APK_Q_FIELD_ORIGIN) | BIT(APK_Q_FIELD_URL) | BIT(APK_Q_FIELD_TAGS) |BIT(APK_Q_FIELD_FILE_SIZE))
#define APK_Q_FIELDS_DEFAULT_IPKG	(APK_Q_FIELDS_DEFAULT_PKG | BIT(APK_Q_FIELD_CONTENTS) | BIT(APK_Q_FIELD_STATUS))

#define APK_Q_FIELDS_ONLY_IPKG \
	(BIT(APK_Q_FIELD_REPLACES) | BIT(APK_Q_FIELD_CONTENTS) | BIT(APK_Q_FIELD_TRIGGERS) | BIT(APK_Q_FIELD_SCRIPTS) | \
	 BIT(APK_Q_FIELD_REPLACES_PRIORITY) | BIT(APK_Q_FIELD_STATUS))

struct apk_query_spec {
	struct {
		uint8_t recursive : 1;
		uint8_t world : 1;
		uint8_t search : 1;
		uint8_t empty_matches_all : 1;
		uint8_t summarize : 1;
	} mode;
	struct {
		uint8_t all_matches : 1;
		uint8_t available : 1;
		uint8_t installed : 1;
		uint8_t orphaned : 1;
		uint8_t upgradable : 1;
		uint8_t revdeps_installed : 1;
	} filter;
	uint8_t revdeps_field;
	uint64_t match;
	uint64_t fields;
	const struct apk_serializer_ops *ser;
};

struct apk_query_match {
	apk_blob_t query;
	apk_blob_t path_target;		// who-owns
	struct apk_name *name;		// name, provider or dependency match
	struct apk_package *pkg;
};

typedef int (*apk_query_match_cb)(void *pctx, struct apk_query_match *);

int apk_query_field_by_name(apk_blob_t k);
uint64_t apk_query_fields(apk_blob_t field_list, uint64_t allowed_fields);
apk_blob_t apk_query_field(int f);
apk_blob_t apk_query_printable_field(apk_blob_t f);
int apk_query_parse_option(struct apk_ctx *ac, int opt, const char *optarg);
extern const char optgroup_query_desc[];

int apk_package_serialize(struct apk_package *pkg, struct apk_database *db, struct apk_query_spec *qs, struct apk_serializer *ser);
int apk_query_match_serialize(struct apk_query_match *qm, struct apk_database *db, struct apk_query_spec *qs, struct apk_serializer *ser);

int apk_query_who_owns(struct apk_database *db, const char *path, struct apk_query_match *qm, char *buf, size_t bufsz);
int apk_query_matches(struct apk_ctx *ac, struct apk_query_spec *qs, struct apk_string_array *args, apk_query_match_cb match, void *pctx);
int apk_query_packages(struct apk_ctx *ac, struct apk_query_spec *qs, struct apk_string_array *args, struct apk_package_array **pkgs);
int apk_query_run(struct apk_ctx *ac, struct apk_query_spec *q, struct apk_string_array *args, struct apk_serializer *ser);
int apk_query_main(struct apk_ctx *ac, struct apk_string_array *args);
