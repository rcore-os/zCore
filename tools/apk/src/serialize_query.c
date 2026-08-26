#include <inttypes.h>
#include "adb.h"
#include "apk_print.h"
#include "apk_query.h"

#define F_OBJECT	BIT(0)
#define F_INDENT	BIT(1)
#define F_SPACE		BIT(2)

#define S_NEW		0
#define S_KEY		1
#define S_SCALAR	2

struct serialize_query {
	struct apk_serializer ser;
	int nest, indent, x;
	uint8_t state;
	uint8_t multiline_value : 1;
	uint8_t first_seen : 1;
	uint8_t flags[APK_SERIALIZE_MAX_NESTING];
};

static void ser_q_write(struct serialize_query *dt, apk_blob_t b)
{
	apk_ostream_write_blob(dt->ser.os, b);
	dt->x += b.len;
}

static void ser_q_start_indent(struct serialize_query *dt, uint8_t flags)
{
	assert(dt->nest < ARRAY_SIZE(dt->flags));
	if (dt->nest == 1) {
		if (dt->first_seen) {
			ser_q_write(dt, APK_BLOB_STRLIT("\n"));
			dt->x = 0;
		}
		dt->first_seen = 1;
	}
	if (flags & F_INDENT) dt->indent++;
	dt->flags[++dt->nest] = flags;
	dt->multiline_value = 0;
}

static int ser_q_start_object(struct apk_serializer *ser, uint32_t schema_id)
{
	struct serialize_query *dt = container_of(ser, struct serialize_query, ser);

	ser_q_start_indent(dt, F_OBJECT);
	return 0;
}

static int ser_q_start_array(struct apk_serializer *ser, int num)
{
	struct serialize_query *dt = container_of(ser, struct serialize_query, ser);
	uint8_t flags = 0;

	if (dt->multiline_value) flags = F_INDENT;
	else if (dt->state == S_KEY) flags = F_SPACE;
	ser_q_start_indent(dt, flags);
	return 0;
}

static int ser_q_end(struct apk_serializer *ser)
{
	struct serialize_query *dt = container_of(ser, struct serialize_query, ser);
	uint8_t flags = dt->flags[dt->nest];

	dt->nest--;
	if (flags & F_INDENT) dt->indent--;
	if ((flags & F_SPACE) || dt->state != S_NEW) {
		apk_ostream_write(dt->ser.os, "\n", 1);
		dt->x = 0;
		dt->state = S_NEW;
	}
	dt->multiline_value = 0;
	return 0;
}

static int ser_q_comment(struct apk_serializer *ser, apk_blob_t comment)
{
	return 0;
}

static void ser_q_item(struct apk_serializer *ser, bool scalar)
{
	struct serialize_query *dt = container_of(ser, struct serialize_query, ser);

	switch (dt->state) {
	case S_KEY:
		apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT(" "));
		break;
	case S_SCALAR:
		if (dt->flags[dt->nest] & F_SPACE) {
			if (dt->x < 80) ser_q_write(dt, APK_BLOB_STRLIT(" "));
			else {
				ser_q_write(dt, APK_BLOB_STRLIT("\n  "));
				dt->x = 2;
			}
		} else {
			ser_q_write(dt, APK_BLOB_STRLIT("\n"));
			dt->x = 0;
		}
		break;
	}
}

static int ser_q_key(struct apk_serializer *ser, apk_blob_t key)
{
	struct serialize_query *dt = container_of(ser, struct serialize_query, ser);

	ser_q_item(ser, false);
	ser_q_write(dt, apk_query_printable_field(key));
	ser_q_write(dt, APK_BLOB_STRLIT(":"));
	dt->state = S_KEY;
	dt->multiline_value =
		apk_query_field(APK_Q_FIELD_CONTENTS).ptr == key.ptr ||
		apk_query_field(APK_Q_FIELD_REPOSITORIES).ptr == key.ptr;
	if (dt->multiline_value) {
		ser_q_write(dt, APK_BLOB_STRLIT("\n"));
		dt->state = S_NEW;
		dt->x = 0;
	}
	return 0;
}

static int ser_q_string(struct apk_serializer *ser, apk_blob_t val, int multiline)
{
	struct serialize_query *dt = container_of(ser, struct serialize_query, ser);
	char pad[] = "                                ";
	apk_blob_t nl = APK_BLOB_STRLIT("\n");

	if (multiline) {
		if (dt->state == S_KEY) apk_ostream_write_blob(dt->ser.os, nl);
		apk_blob_foreach_token(line, val, nl) {
			ser_q_write(dt, APK_BLOB_STRLIT("  "));
			ser_q_write(dt, line);
			ser_q_write(dt, nl);
		}
		dt->state = S_NEW;
		dt->x = 0;
	} else {
		ser_q_item(ser, true);
		if (dt->indent) ser_q_write(dt, APK_BLOB_PTR_LEN(pad, dt->indent*2));
		ser_q_write(dt, val);
		dt->state = S_SCALAR;
	}
	return 0;
}

static int ser_q_numeric(struct apk_serializer *ser, uint64_t val, int hint)
{
	struct serialize_query *dt = container_of(ser, struct serialize_query, ser);
	char buf[64];

	ser_q_item(ser, true);
	ser_q_write(dt, apk_ser_format_numeric(ser, buf, sizeof buf, val, hint));
	dt->state = S_SCALAR;
	return 0;
}

const struct apk_serializer_ops apk_serializer_query = {
	.context_size = sizeof(struct serialize_query),
	.start_object = ser_q_start_object,
	.start_array = ser_q_start_array,
	.end = ser_q_end,
	.comment = ser_q_comment,
	.key = ser_q_key,
	.string = ser_q_string,
	.numeric = ser_q_numeric,
};
