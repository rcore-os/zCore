/* apk_serialize.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2025 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include "apk_blob.h"

#define APK_SERIALIZE_MAX_NESTING 32

#define APK_SERIALIZE_INT	0
#define APK_SERIALIZE_OCTAL	1
#define APK_SERIALIZE_SIZE	2
#define APK_SERIALIZE_TIME	3

struct apk_ctx;
struct apk_serializer;
struct apk_ostream;
struct apk_trust;

struct apk_serializer_ops {
	size_t context_size;
	int (*init)(struct apk_serializer *);
	void (*cleanup)(struct apk_serializer *);
	int (*start_object)(struct apk_serializer *, uint32_t sechema_id);
	int (*start_array)(struct apk_serializer *, int num_items);
	int (*end)(struct apk_serializer *);
	int (*comment)(struct apk_serializer *, apk_blob_t comment);
	int (*key)(struct apk_serializer *, apk_blob_t key_name);
	int (*string)(struct apk_serializer *, apk_blob_t val, int multiline);
	int (*numeric)(struct apk_serializer *, uint64_t val, int hint);
};

extern const struct apk_serializer_ops apk_serializer_yaml, apk_serializer_json, apk_serializer_query;

struct apk_serializer {
	const struct apk_serializer_ops *ops;
	struct apk_ostream *os;
	struct apk_trust *trust;
	unsigned int pretty_print : 1;
};

const struct apk_serializer_ops *apk_serializer_lookup(const char *format, const struct apk_serializer_ops *def);
struct apk_serializer *_apk_serializer_init(const struct apk_ctx *ac, const struct apk_serializer_ops *ops, struct apk_ostream *os, void *ctx);
#define apk_serializer_init_alloca(ac, ops, os) _apk_serializer_init(ac, ops, os, (ops)->context_size < 1024 ? alloca((ops)->context_size) : NULL)
void apk_serializer_cleanup(struct apk_serializer *ser);

apk_blob_t apk_ser_format_numeric(struct apk_serializer *ser, char *buf, size_t sz, uint64_t val, int hint);

static inline int apk_ser_start_schema(struct apk_serializer *ser, uint32_t schema_id) { return ser->ops->start_object(ser, schema_id); }
static inline int apk_ser_start_object(struct apk_serializer *ser) { return ser->ops->start_object(ser, 0); }
static inline int apk_ser_start_array(struct apk_serializer *ser, unsigned int num) { return ser->ops->start_array(ser, num); }
static inline int apk_ser_end(struct apk_serializer *ser) { return ser->ops->end(ser); }
static inline int apk_ser_comment(struct apk_serializer *ser, apk_blob_t comment) { return ser->ops->comment(ser, comment); }
static inline int apk_ser_key(struct apk_serializer *ser, apk_blob_t key_name) { return ser->ops->key(ser, key_name); }
static inline int apk_ser_string_ml(struct apk_serializer *ser, apk_blob_t val, int ml) { return ser->ops->string(ser, val, ml); }
static inline int apk_ser_string(struct apk_serializer *ser, apk_blob_t val) { return ser->ops->string(ser, val, 0); }
static inline int apk_ser_numeric(struct apk_serializer *ser, uint64_t val, int hint) { return ser->ops->numeric(ser, val, hint); }
