#pragma once

#include <endian.h>
#include <stdint.h>
#include <sys/types.h>
#include "apk_io.h"
#include "apk_trust.h"
#include "apk_serialize.h"

struct apk_extract_ctx;
struct adb;
struct adb_obj;
struct adb_verify_ctx;

typedef uint32_t adb_val_t;

#define ADB_TYPE_SPECIAL	0x00000000
#define ADB_TYPE_INT		0x10000000
#define ADB_TYPE_INT_32		0x20000000
#define ADB_TYPE_INT_64		0x30000000
#define ADB_TYPE_BLOB_8		0x80000000
#define ADB_TYPE_BLOB_16	0x90000000
#define ADB_TYPE_BLOB_32	0xa0000000
#define ADB_TYPE_ARRAY		0xd0000000
#define ADB_TYPE_OBJECT		0xe0000000
#define ADB_TYPE_ERROR		0xf0000000
#define ADB_TYPE_MASK		0xf0000000
#define ADB_VALUE_MASK		0x0fffffff
#define ADB_VAL_TYPE(x)		((le32toh(x))&ADB_TYPE_MASK)
#define ADB_VAL_VALUE(x)	((le32toh(x))&ADB_VALUE_MASK)
#define ADB_IS_ERROR(x)		(ADB_VAL_TYPE(x) == ADB_TYPE_ERROR)
#define ADB_VAL(type, val)	(htole32((type) | (val)))
#define ADB_ERROR(val)		ADB_VAL(ADB_TYPE_ERROR, val)

/* ADB_TYPE_SPECIAL */
#define ADB_VAL_NULL		0x00000000
#define ADB_VAL_TRUE		0x00000001
#define ADB_VAL_FALSE		0x00000002

#define ADB_NULL		ADB_VAL(ADB_TYPE_SPECIAL, ADB_VAL_NULL)

/* Generic */
#define ADBI_NUM_ENTRIES	0x00
#define ADBI_FIRST		0x01

/* File Header */
#define ADB_FORMAT_MAGIC	0x2e424441	// ADB.
#define ADB_SCHEMA_ANY		0
#define ADB_SCHEMA_IMPLIED	0x80000000

struct adb_file_header {
	uint32_t magic;
	uint32_t schema;
};

/* Blocks */
#define ADB_BLOCK_ALIGNMENT	8
#define ADB_BLOCK_ADB		0
#define ADB_BLOCK_SIG		1
#define ADB_BLOCK_DATA		2
#define ADB_BLOCK_EXT		3
#define ADB_BLOCK_MAX		4

struct adb_block {
	uint32_t type_size;
	uint32_t reserved;
	uint64_t x_size;
};

static inline struct adb_block adb_block_init(uint32_t type, uint64_t length) {
	if (length <= 0x3fffffff - sizeof(uint32_t)) {
		return (struct adb_block) {
			.type_size = htole32((type << 30) + sizeof(uint32_t) + length),
		};
	}
	return (struct adb_block) {
		.type_size = htole32((ADB_BLOCK_EXT << 30) + type),
		.x_size = htole64(sizeof(struct adb_block) + length),
	};
}
static inline bool adb_block_is_ext(struct adb_block *b) {
	return (le32toh((b)->type_size) >> 30) == ADB_BLOCK_EXT;
}
static inline uint32_t adb_block_type(struct adb_block *b) {
	return adb_block_is_ext(b) ? (le32toh(b->type_size) & 0x3fffffff) : (le32toh(b->type_size) >> 30);
}
static inline uint64_t adb_block_rawsize(struct adb_block *b) {
	return adb_block_is_ext(b) ? le64toh(b->x_size) : (le32toh(b->type_size) & 0x3fffffff);
}
static inline uint32_t adb_block_hdrsize(struct adb_block *b) {
	return adb_block_is_ext(b) ? sizeof *b : sizeof b->type_size;
}
static inline uint64_t adb_block_size(struct adb_block *b) { return ROUND_UP(adb_block_rawsize(b), ADB_BLOCK_ALIGNMENT); }
static inline uint64_t adb_block_length(struct adb_block *b) { return adb_block_rawsize(b) - adb_block_hdrsize(b); }
static inline uint32_t adb_block_padding(struct adb_block *b) { return adb_block_size(b) - adb_block_rawsize(b); }
static inline void *adb_block_payload(struct adb_block *b) { return (char*)b + adb_block_hdrsize(b); }
static inline apk_blob_t adb_block_blob(struct adb_block *b) {
	return APK_BLOB_PTR_LEN(adb_block_payload(b), adb_block_length(b));
}

#define ADB_MAX_SIGNATURE_LEN 2048

struct adb_hdr {
	uint8_t adb_compat_ver;
	uint8_t adb_ver;
	uint16_t reserved;
	adb_val_t root;
};

struct adb_sign_hdr {
	uint8_t sign_ver, hash_alg;
};

struct adb_sign_v0 {
	struct adb_sign_hdr hdr;
	uint8_t id[16];
	uint8_t sig[];
};

/* Schema */
#define ADB_KIND_ADB		1
#define ADB_KIND_OBJECT		2
#define ADB_KIND_ARRAY		3
#define ADB_KIND_BLOB		4
#define ADB_KIND_NUMERIC	5

#define ADB_ARRAY_ITEM(_t) (const struct adb_object_schema_field[1]) { {.kind = &(_t).kind} }
#define ADB_OBJECT_FIELDS(n) (const struct adb_object_schema_field[n])
#define ADB_FIELD(_i, _n, _t) [(_i)-1] = { .name = _n, .kind = &(_t).kind }

#define ADB_OBJCMP_EXACT	0	// match all fields
#define ADB_OBJCMP_TEMPLATE	1	// match fields set on template
#define ADB_OBJCMP_INDEX	2	// match fields until first non-set one

struct adb_object_schema_field {
	const char *name;
	const uint8_t *kind;
};

struct adb_object_schema {
	uint8_t kind;
	uint16_t num_fields;
	uint16_t num_compare;

	apk_blob_t (*tostring)(struct adb_obj *, char *, size_t);
	int (*fromstring)(struct adb_obj *, apk_blob_t);
	void (*pre_commit)(struct adb_obj *);
	const struct adb_object_schema_field *fields;
};

struct adb_scalar_schema {
	uint8_t kind;
	uint8_t hint : 4;
	uint8_t multiline : 1;

	apk_blob_t (*tostring)(struct adb*, adb_val_t, char *, size_t);
	adb_val_t (*fromstring)(struct adb*, apk_blob_t);
	int (*compare)(struct adb*, adb_val_t, struct adb*, adb_val_t);
};

struct adb_adb_schema {
	uint8_t kind;
	uint32_t schema_id;
	const struct adb_object_schema *schema;
};

/* Database read interface */
struct adb_w_bucket {
	struct list_head node;
	struct adb_w_bucket_entry {
		uint32_t hash;
		uint32_t offs;
		uint32_t len;
	} entries[40];
};

struct adb {
	struct apk_istream *is;
	apk_blob_t adb;
	uint32_t schema;
	uint32_t num_buckets;
	uint32_t alloc_len;
	uint8_t no_cache;
	struct list_head *bucket;
};

struct adb_obj {
	struct adb *db;
	const struct adb_object_schema *schema;
	adb_val_t *obj;
	uint32_t num;
	uint32_t dynamic : 1;
};

/* Container read interface */
static inline void adb_init(struct adb *db) { memset(db, 0, sizeof *db); }
int adb_free(struct adb *);
void adb_reset(struct adb *);

int adb_m_blob(struct adb *, apk_blob_t, struct apk_trust *);
int adb_m_process(struct adb *db, struct apk_istream *is, uint32_t expected_schema, struct apk_trust *trust, struct apk_extract_ctx *ectx, int (*cb)(struct adb *, struct adb_block *, struct apk_istream *));
static inline int adb_m_open(struct adb *db, struct apk_istream *is, uint32_t expected_schema, struct apk_trust *trust) {
	return adb_m_process(db, is, expected_schema, trust, NULL, 0);
}
#define adb_w_init_alloca(db, schema, num_buckets) adb_w_init_dynamic(db, schema, alloca(sizeof(struct list_head[num_buckets])), num_buckets)
#define adb_w_init_tmp(db, size) adb_w_init_static(db, alloca(size), size)
int adb_w_init_dynamic(struct adb *db, uint32_t schema, void *buckets, size_t num_buckets);
int adb_w_init_static(struct adb *db, void *buf, size_t bufsz);

/* Primitive read */
adb_val_t adb_r_root(const struct adb *);
struct adb_obj *adb_r_rootobj(struct adb *a, struct adb_obj *o, const struct adb_object_schema *);
uint64_t adb_r_int(const struct adb *, adb_val_t);
apk_blob_t adb_r_blob(const struct adb *, adb_val_t);
struct adb_obj *adb_r_obj(struct adb *, adb_val_t, struct adb_obj *o, const struct adb_object_schema *);

/* Object read */
static inline uint32_t adb_ro_num(const struct adb_obj *o) { return o->num; }
static inline uint32_t adb_ra_num(const struct adb_obj *o) { return (o->num ?: 1) - 1; }

const uint8_t *adb_ro_kind(const struct adb_obj *o, unsigned i);
adb_val_t adb_ro_val(const struct adb_obj *o, unsigned i);
uint64_t adb_ro_int(const struct adb_obj *o, unsigned i);
apk_blob_t adb_ro_blob(const struct adb_obj *o, unsigned i);
struct adb_obj *adb_ro_obj(const struct adb_obj *o, unsigned i, struct adb_obj *);
int adb_ro_cmpobj(const struct adb_obj *o1, const struct adb_obj *o2, unsigned mode);
int adb_ro_cmp(const struct adb_obj *o1, const struct adb_obj *o2, unsigned i, unsigned mode);
int adb_ra_find(struct adb_obj *arr, int cur, struct adb_obj *tmpl);

/* Primitive write */
void adb_w_root(struct adb *, adb_val_t);
void adb_w_rootobj(struct adb_obj *);
adb_val_t adb_w_blob_vec(struct adb *, uint32_t, apk_blob_t *);
adb_val_t adb_w_blob(struct adb *, apk_blob_t);
adb_val_t adb_w_int(struct adb *, uint64_t);
adb_val_t adb_w_copy(struct adb *, struct adb *, adb_val_t);
adb_val_t adb_w_adb(struct adb *, struct adb *);
adb_val_t adb_w_fromstring(struct adb *, const uint8_t *kind, apk_blob_t);

/* Object write */
#define adb_wo_alloca(o, schema, db) adb_wo_init(o, alloca(sizeof(adb_val_t[(schema)->num_fields])), schema, db)

struct adb_obj *adb_wo_init(struct adb_obj *, adb_val_t *, const struct adb_object_schema *, struct adb *);
struct adb_obj *adb_wo_init_val(struct adb_obj *, adb_val_t *, const struct adb_obj *, unsigned i);
void adb_wo_free(struct adb_obj *);
void adb_wo_reset(struct adb_obj *);
void adb_wo_resetdb(struct adb_obj *);
adb_val_t adb_w_obj(struct adb_obj *);
adb_val_t adb_w_arr(struct adb_obj *);
int adb_wo_fromstring(struct adb_obj *o, apk_blob_t);
int adb_wo_copyobj(struct adb_obj *o, struct adb_obj *);
adb_val_t adb_wo_val(struct adb_obj *o, unsigned i, adb_val_t);
adb_val_t adb_wo_val_fromstring(struct adb_obj *o, unsigned i, apk_blob_t);
adb_val_t adb_wo_int(struct adb_obj *o, unsigned i, uint64_t);
adb_val_t adb_wo_blob(struct adb_obj *o, unsigned i, apk_blob_t);
adb_val_t adb_wo_blob_raw(struct adb_obj *o, unsigned i, apk_blob_t);
adb_val_t adb_wo_obj(struct adb_obj *o, unsigned i, struct adb_obj *);
adb_val_t adb_wo_arr(struct adb_obj *o, unsigned i, struct adb_obj *);
adb_val_t adb_wa_append(struct adb_obj *o, adb_val_t);
adb_val_t adb_wa_append_obj(struct adb_obj *o, struct adb_obj *);
adb_val_t adb_wa_append_fromstring(struct adb_obj *o, apk_blob_t);
void adb_wa_sort(struct adb_obj *);
void adb_wa_sort_unique(struct adb_obj *);

/* Schema helpers */
int adb_s_field_by_name_blob(const struct adb_object_schema *schema, apk_blob_t blob);
int adb_s_field_by_name(const struct adb_object_schema *, const char *);
int adb_s_field_subst(void *ctx, apk_blob_t var, apk_blob_t *to);

/* Creation */
int adb_c_header(struct apk_ostream *os, struct adb *db);
int adb_c_block(struct apk_ostream *os, uint32_t type, apk_blob_t);
int adb_c_block_data(struct apk_ostream *os, apk_blob_t hdr, uint64_t size, struct apk_istream *is);
int adb_c_block_copy(struct apk_ostream *os, struct adb_block *b, struct apk_istream *is, struct adb_verify_ctx *);
int adb_c_adb(struct apk_ostream *os, struct adb *db, struct apk_trust *t);
int adb_c_create(struct apk_ostream *os, struct adb *db, struct apk_trust *t);

/* Trust */
struct adb_verify_ctx {
	uint32_t calc;
	struct apk_digest sha256;
	struct apk_digest sha512;
};

int adb_trust_write_signatures(struct apk_trust *trust, struct adb *db, struct adb_verify_ctx *vfy, struct apk_ostream *os);
int adb_trust_verify_signature(struct apk_trust *trust, struct adb *db, struct adb_verify_ctx *vfy, apk_blob_t sigb);

/* SAX style event based handling of ADB */

struct adb_db_schema {
	unsigned long magic;
	const struct adb_object_schema *root;
};

extern const struct adb_db_schema adb_all_schemas[];

int adb_walk_adb(struct apk_istream *is, struct apk_ostream *os, const struct apk_serializer_ops *ser, struct apk_ctx *ac);

// Seamless compression support

struct adb_compression_spec {
	uint8_t alg;
	uint8_t level;
};

// Internally, "none" compression is treated specially:
// none/0 means "default compression"
// none/1 is "no compression"
#define ADB_COMP_NONE		0x00
#define ADB_COMP_DEFLATE	0x01
#define ADB_COMP_ZSTD		0x02

int adb_parse_compression(const char *spec_string, struct adb_compression_spec *spec);
struct apk_istream *adb_decompress(struct apk_istream *is, struct adb_compression_spec *spec);
struct apk_ostream *adb_compress(struct apk_ostream *os, struct adb_compression_spec *spec);
