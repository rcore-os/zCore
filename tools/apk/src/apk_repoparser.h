#pragma once
#include "apk_blob.h"
#include "apk_hash.h"

struct apk_out;
struct apk_repoparser;

struct apk_repoparser_ops {
	int (*repository)(struct apk_repoparser *rp, apk_blob_t url, const char *index_file, apk_blob_t tag);
};

struct apk_repoparser {
	struct apk_out *out;
	struct apk_hash variables;
	const struct apk_repoparser_ops *ops;
	const char *file;
	int line;
};

#define APK_VARF_OVERWRITE 1
#define APK_VARF_READONLY 2

int apk_variable_set(struct apk_hash *vars, apk_blob_t key, apk_blob_t value, uint8_t flags);

void apk_repoparser_init(struct apk_repoparser *rp, struct apk_out *out, const struct apk_repoparser_ops *ops);
void apk_repoparser_free(struct apk_repoparser *rp);
void apk_repoparser_set_file(struct apk_repoparser *rp, const char *file);
int apk_repoparser_parse(struct apk_repoparser *rp, apk_blob_t line, bool allow_keywords);
