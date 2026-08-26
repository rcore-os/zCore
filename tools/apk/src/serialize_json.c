#include "adb.h"
#include "apk_print.h"

struct serialize_json {
	struct apk_serializer ser;
	int nest;
	unsigned int key_printed : 1;
	unsigned int need_separator : 1;
	unsigned int need_newline : 1;
	char end[APK_SERIALIZE_MAX_NESTING];
};

static void ser_json_indent(struct serialize_json *dt, bool item)
{
	static char pad[] = "\n                                ";

	if (dt->key_printed) {
		apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT(" "));
	} else {
		if (item && dt->need_separator) apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT(","));
		if (dt->need_newline) {
			assert(sizeof pad >= 2*dt->nest);
			apk_ostream_write(dt->ser.os, pad, 1 + 2*dt->nest);
		} else {
			apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT(" "));
		}
	}
	dt->key_printed = 0;
}

static void ser_json_start_indent(struct serialize_json *dt, char start_brace, char end_brace)
{
	assert(dt->nest < ARRAY_SIZE(dt->end));
	apk_ostream_write(dt->ser.os, &start_brace, 1);
	dt->end[++dt->nest] = end_brace;
	dt->need_separator = 0;
	dt->need_newline = 1;
}

static int ser_json_start_object(struct apk_serializer *ser, uint32_t schema_id)
{
	struct serialize_json *dt = container_of(ser, struct serialize_json, ser);

	if (dt->nest) ser_json_indent(dt, true);
	ser_json_start_indent(dt, '{', '}');
	return 0;
}

static int ser_json_start_array(struct apk_serializer *ser, int num)
{
	struct serialize_json *dt = container_of(ser, struct serialize_json, ser);

	if (dt->nest) ser_json_indent(dt, true);
	ser_json_start_indent(dt, '[', ']');
	return 0;
}

static int ser_json_end(struct apk_serializer *ser)
{
	struct serialize_json *dt = container_of(ser, struct serialize_json, ser);

	dt->need_newline = 1;
	dt->nest--;
	ser_json_indent(dt, false);
	apk_ostream_write(dt->ser.os, &dt->end[dt->nest+1], 1);
	dt->end[dt->nest+1] = 0;
	dt->need_separator = 1;
	dt->need_newline = 0;
	if (!dt->nest) apk_ostream_write(dt->ser.os, "\n", 1);
	return 0;
}

static int ser_json_comment(struct apk_serializer *ser, apk_blob_t comment)
{
	// JSON is data only and does not allow comments
	return 0;
}

static int ser_json_key(struct apk_serializer *ser, apk_blob_t key)
{
	struct serialize_json *dt = container_of(ser, struct serialize_json, ser);

	dt->need_newline = 1;
	ser_json_indent(dt, true);
	apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT("\""));
	apk_ostream_write_blob(dt->ser.os, key);
	apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT("\":"));
	dt->key_printed = 1;
	dt->need_separator = 1;
	return 0;
}

static int ser_json_string(struct apk_serializer *ser, apk_blob_t val, int multiline)
{
	struct serialize_json *dt = container_of(ser, struct serialize_json, ser);
	char esc[2] = "\\ ";
	int done = 0;

	dt->need_newline = 1;
	ser_json_indent(dt, true);
	apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT("\""));
	for (int i = 0; i < val.len; i++) {
		char ch = val.ptr[i];
		switch (ch) {
		case '"': esc[1] = '"'; break;
		case '\n': esc[1] = 'n'; break;
		case '\t': esc[1] = 't'; break;
		case '\\': esc[1] = '\\'; break;
		default: continue;
		}
		if (i != done) apk_ostream_write(dt->ser.os, &val.ptr[done], i - done);
		apk_ostream_write(dt->ser.os, esc, sizeof esc);
		done = i+1;
	}
	if (done < val.len) apk_ostream_write(dt->ser.os, &val.ptr[done], val.len - done);
	apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT("\""));
	dt->need_separator = 1;
	return 0;
}

static int ser_json_numeric(struct apk_serializer *ser, uint64_t val, int hint)
{
	struct serialize_json *dt = container_of(ser, struct serialize_json, ser);

	dt->need_newline = 1;
	ser_json_indent(dt, true);
	apk_ostream_fmt(dt->ser.os, "%llu", val);
	dt->need_separator = 1;
	return 0;
}

const struct apk_serializer_ops apk_serializer_json = {
	.context_size = sizeof(struct serialize_json),
	.start_object = ser_json_start_object,
	.start_array = ser_json_start_array,
	.end = ser_json_end,
	.comment = ser_json_comment,
	.key = ser_json_key,
	.string = ser_json_string,
	.numeric = ser_json_numeric,
};
