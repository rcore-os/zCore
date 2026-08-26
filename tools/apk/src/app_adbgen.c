#include <errno.h>
#include <unistd.h>

#include "adb.h"
#include "apk_adb.h"
#include "apk_applet.h"
#include "apk_print.h"

//#define DEBUG_PRINT
#ifdef DEBUG_PRINT
#include <stdio.h>
#define dbg_printf(args...) fprintf(stderr, args)
#else
#define dbg_printf(args...)
#endif

#define SERIALIZE_ADB_MAX_IDB		2
#define SERIALIZE_ADB_MAX_VALUES	100000

struct serialize_adb {
	struct apk_serializer ser;

	struct adb db;
	struct adb idb[SERIALIZE_ADB_MAX_IDB];
	int nest, nestdb, num_vals;
	struct adb_obj objs[APK_SERIALIZE_MAX_NESTING];
	unsigned int curkey[APK_SERIALIZE_MAX_NESTING];
	adb_val_t vals[SERIALIZE_ADB_MAX_VALUES];

	struct list_head db_buckets[1000];
	struct list_head idb_buckets[100];
};

static int ser_adb_init(struct apk_serializer *ser)
{
	struct serialize_adb *dt = container_of(ser, struct serialize_adb, ser);

	adb_w_init_dynamic(&dt->db, 0, dt->db_buckets, ARRAY_SIZE(dt->db_buckets));
	adb_w_init_dynamic(&dt->idb[0], 0, dt->idb_buckets, ARRAY_SIZE(dt->idb_buckets));
	return 0;
}

static void ser_adb_cleanup(struct apk_serializer *ser)
{
	struct serialize_adb *dt = container_of(ser, struct serialize_adb, ser);

	adb_free(&dt->db);
	adb_free(&dt->idb[0]);
}

static int ser_adb_start_object(struct apk_serializer *ser, uint32_t schema_id)
{
	struct serialize_adb *dt = container_of(ser, struct serialize_adb, ser);

	if (dt->db.schema == 0) {
		const struct adb_db_schema *s;
		dt->db.schema = schema_id;
		for (s = adb_all_schemas; s->magic; s++)
			if (s->magic == schema_id) break;
		if (!s || !s->magic) return -APKE_ADB_SCHEMA;

		adb_wo_init(&dt->objs[0], &dt->vals[0], s->root, &dt->db);
		dt->num_vals += s->root->num_fields;
	} else {
		if (!dt->db.schema) return -APKE_ADB_SCHEMA;
		if (dt->nest >= ARRAY_SIZE(dt->objs)) return -APKE_ADB_LIMIT;
		if (dt->curkey[dt->nest] == 0 &&
		    dt->objs[dt->nest].schema->kind == ADB_KIND_OBJECT)
			return -APKE_ADB_SCHEMA;

		dt->nest++;
		adb_wo_init_val(
			&dt->objs[dt->nest], &dt->vals[dt->num_vals],
			&dt->objs[dt->nest-1], dt->curkey[dt->nest-1]);

		if (*adb_ro_kind(&dt->objs[dt->nest-1], dt->curkey[dt->nest-1]) == ADB_KIND_ADB) {
			struct adb_adb_schema *schema = container_of(&dt->objs[dt->nest-1].schema->kind, struct adb_adb_schema, kind);
			if (dt->nestdb >= ARRAY_SIZE(dt->idb)) return -APKE_ADB_LIMIT;
			adb_reset(&dt->idb[dt->nestdb]);
			dt->idb[dt->nestdb].schema = schema->schema_id;
			dt->objs[dt->nest].db = &dt->idb[dt->nestdb];
			dt->nestdb++;
		}
		dt->num_vals += dt->objs[dt->nest].schema->num_fields;
	}
	if (dt->num_vals >= ARRAY_SIZE(dt->vals)) return -APKE_ADB_LIMIT;
	return 0;
}

static int ser_adb_start_array(struct apk_serializer *ser, int num)
{
	return ser_adb_start_object(ser, 0);
}

static int ser_adb_end(struct apk_serializer *ser)
{
	struct serialize_adb *dt = container_of(ser, struct serialize_adb, ser);
	adb_val_t val;

	val = adb_w_obj(&dt->objs[dt->nest]);
	adb_wo_free(&dt->objs[dt->nest]);
	if (ADB_IS_ERROR(val))
		return -ADB_VAL_VALUE(val);

	dt->curkey[dt->nest] = 0;
	dt->num_vals -= dt->objs[dt->nest].schema->num_fields;

	if (dt->nest == 0) {
		adb_w_root(&dt->db, val);
		int r = adb_c_create(dt->ser.os, &dt->db, dt->ser.trust);
		dt->ser.os = NULL;
		return r;
	}

	dt->nest--;

	if (*adb_ro_kind(&dt->objs[dt->nest], dt->curkey[dt->nest]) == ADB_KIND_ADB) {
		dt->nestdb--;
		adb_w_root(&dt->idb[dt->nestdb], val);
		val = adb_w_adb(&dt->db, &dt->idb[dt->nestdb]);
	}

	if (dt->curkey[dt->nest] == 0) {
		adb_wa_append(&dt->objs[dt->nest], val);
	} else {
		adb_wo_val(&dt->objs[dt->nest], dt->curkey[dt->nest], val);
		dt->curkey[dt->nest] = 0;
	}

	return 0;
}

static int ser_adb_comment(struct apk_serializer *ser, apk_blob_t comment)
{
	return 0;
}

static int ser_adb_key(struct apk_serializer *ser, apk_blob_t key)
{
	struct serialize_adb *dt = container_of(ser, struct serialize_adb, ser);
	uint8_t kind = dt->objs[dt->nest].schema->kind;

	if (kind != ADB_KIND_OBJECT && kind != ADB_KIND_ADB)
		return -APKE_ADB_SCHEMA;

	dt->curkey[dt->nest] = adb_s_field_by_name_blob(dt->objs[dt->nest].schema, key);
	if (dt->curkey[dt->nest] == 0)
		return -APKE_ADB_SCHEMA;

	return 0;
}

static int ser_adb_string(struct apk_serializer *ser, apk_blob_t scalar, int multiline)
{
	struct serialize_adb *dt = container_of(ser, struct serialize_adb, ser);

	if (dt->objs[dt->nest].schema->kind == ADB_KIND_ARRAY) {
		adb_wa_append_fromstring(&dt->objs[dt->nest], scalar);
	} else {
		if (dt->curkey[dt->nest] == 0)
			adb_wo_fromstring(&dt->objs[dt->nest], scalar);
		else
			adb_wo_val_fromstring(&dt->objs[dt->nest], dt->curkey[dt->nest], scalar);
	}
	dt->curkey[dt->nest] = 0;

	return 0;
}

const struct apk_serializer_ops apk_serializer_adb = {
	.context_size = sizeof(struct serialize_adb),
	.init = ser_adb_init,
	.cleanup = ser_adb_cleanup,
	.start_object = ser_adb_start_object,
	.start_array = ser_adb_start_array,
	.end = ser_adb_end,
	.comment = ser_adb_comment,
	.key = ser_adb_key,
	.string = ser_adb_string,
};

static int adb_walk_yaml(struct apk_ctx *ac, struct apk_istream *is, struct apk_ostream *os, const struct apk_serializer_ops *ops, struct apk_trust *trust)
{
	const apk_blob_t token = APK_BLOB_STR("\n");
	const apk_blob_t comment = APK_BLOB_STR(" #");
	const apk_blob_t key_sep = APK_BLOB_STR(": ");
	struct apk_serializer *ser;
	char mblockdata[1024*4];
	apk_blob_t l, comm, mblock = APK_BLOB_BUF(mblockdata);
	int r = 0, i, multi_line = 0, nesting = 0, new_item = 0;
	uint8_t started[64] = {0};

	ser = apk_serializer_init_alloca(ac, ops, os);
	if (IS_ERR(ser)) {
		if (IS_ERR(is)) apk_istream_close(is);
		return PTR_ERR(ser);
	}
	if (IS_ERR(is)) {
		r = PTR_ERR(is);
		goto err;
	}
	ser->trust = trust;

	if (apk_istream_get_delim(is, token, &l) != 0) goto err;
	if (!apk_blob_pull_blob_match(&l, APK_BLOB_STR("#%SCHEMA: "))) goto err;
	if ((r = apk_ser_start_schema(ser, apk_blob_pull_uint(&l, 16))) != 0) goto err;

	started[0] = 1;
	while (apk_istream_get_delim(is, token, &l) == 0) {
		for (i = 0; l.len >= 2 && l.ptr[0] == ' ' && l.ptr[1] == ' '; i++, l.ptr += 2, l.len -= 2)
			if (multi_line && i >= multi_line) break;

		for (; nesting > i; nesting--) {
			if (multi_line) {
				apk_blob_t data = apk_blob_pushed(APK_BLOB_BUF(mblockdata), mblock);
				if (APK_BLOB_IS_NULL(data)) {
					r = -E2BIG;
					goto err;
				}
				if (data.len && data.ptr[data.len-1] == '\n') data.len--;
				dbg_printf("Multiline-Scalar >%d> "BLOB_FMT"\n", nesting, BLOB_PRINTF(data));
				if ((r = apk_ser_string_ml(ser, data, 1)) != 0) goto err;
				mblock = APK_BLOB_BUF(mblockdata);
				multi_line = 0;
			}
			if (started[nesting]) {
				dbg_printf("End %d\n", nesting);
				if ((r = apk_ser_end(ser)) != 0) goto err;
			}
		}
		if (l.len >= 2 && l.ptr[0] == '-' && l.ptr[1] == ' ') {
			l.ptr += 2, l.len -= 2;
			if (!started[nesting]) {
				dbg_printf("Array %d\n", nesting);
				if ((r = apk_ser_start_array(ser, 0)) != 0) goto err;
				started[nesting] = 1;
			}
			new_item = 1;
		}
		dbg_printf(" >%d/%d< >"BLOB_FMT"<\n", nesting, i, BLOB_PRINTF(l));

		if (multi_line) {
			dbg_printf("Scalar-Block:>%d> "BLOB_FMT"\n", nesting, BLOB_PRINTF(l));
			apk_blob_push_blob(&mblock, l);
			apk_blob_push_blob(&mblock, APK_BLOB_STR("\n"));
			new_item = 0;
			continue;
		}

		if (l.len && l.ptr[0] == '#') {
			if ((r = apk_ser_comment(ser, l)) != 0) goto err;
			continue;
		}

		// contains ' #' -> comment
		if (!apk_blob_split(l, comment, &l, &comm))
			comm.len = 0;

		if (l.len) {
			apk_blob_t key = APK_BLOB_NULL, scalar = APK_BLOB_NULL;
			int start = 0;

			if (apk_blob_split(l, key_sep, &key, &scalar)) {
				// contains ': ' -> key + scalar
			} else if (l.ptr[l.len-1] == ':') {
				// ends ':' -> key + indented object/array
				key = APK_BLOB_PTR_LEN(l.ptr, l.len-1);
				start = 1;
			} else {
				scalar = l;
			}
			if (key.len) {
				if (new_item) {
					started[++nesting] = 0;
					dbg_printf("Array-Object %d\n", nesting);
				}
				if (!started[nesting]) {
					dbg_printf("Object %d\n", nesting);
					if ((r = apk_ser_start_object(ser)) != 0) goto err;
					started[nesting] = 1;
				}
				dbg_printf("Key >%d> "BLOB_FMT"\n", nesting, BLOB_PRINTF(key));
				if ((r = apk_ser_key(ser, key)) != 0) goto err;
				if (start) started[++nesting] = 0;
			}

			if (scalar.len) {
				if (scalar.ptr[0] == '|') {
					dbg_printf("Scalar-block >%d> "BLOB_FMT"\n", nesting, BLOB_PRINTF(scalar));
					// scalar '|' -> starts string literal block
					started[++nesting] = 0;
					multi_line = nesting;
				} else {
					if (scalar.ptr[0] == '\'') {
						dbg_printf("Scalar-squote >%d> "BLOB_FMT"\n", nesting, BLOB_PRINTF(scalar));
						if (scalar.len < 2 || scalar.ptr[scalar.len-1] != '\'') {
							r = -APKE_FORMAT_INVALID;
							goto err;
						}
						scalar.ptr ++;
						scalar.len -= 2;
					} else {
						dbg_printf("Scalar >%d> "BLOB_FMT"\n", nesting, BLOB_PRINTF(scalar));
					}
					if ((r = apk_ser_string(ser, scalar)) != 0) goto err;
				}
			}
			new_item = 0;
		}

		if (comm.len) {
			if ((r = apk_ser_comment(ser, comm)) != 0) goto err;
		}

		dbg_printf(">%d> "BLOB_FMT"\n", nesting, BLOB_PRINTF(l));
	}
	apk_ser_end(ser);

err:
	apk_serializer_cleanup(ser);
	return apk_istream_close_error(is, r);
}

static int adbgen_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_out *out = &ac->out;

	apk_array_foreach_item(arg, args) {
		int r = adb_walk_yaml(ac,
			apk_istream_from_file(AT_FDCWD, arg),
			apk_ostream_to_fd(STDOUT_FILENO),
			&apk_serializer_adb,
			apk_ctx_get_trust(ac));
		if (r) {
			apk_err(out, "%s: %s", arg, apk_error_str(r));
			return r;
		}
	}

	return 0;
}

static struct apk_applet apk_adbgen = {
	.name = "adbgen",
	.main = adbgen_main,
};
APK_DEFINE_APPLET(apk_adbgen);

