#include "adb.h"
#include "apk_print.h"

#define F_ARRAY	1

struct serialize_yaml {
	struct apk_serializer ser;
	int nest, indent;
	unsigned int line_started : 1;
	unsigned int key_printed : 1;
	uint8_t flags[APK_SERIALIZE_MAX_NESTING];
};

static void ser_yaml_indent(struct serialize_yaml *dt, bool item, bool continue_line)
{
	char pad[] = "                                ";

	if (!dt->line_started) {
		assert(sizeof pad >= 2*dt->indent);
		apk_ostream_write(dt->ser.os, pad, 2*dt->indent);

		if (item && (dt->flags[dt->nest]&F_ARRAY))
			apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT("- "));
	} else if (dt->key_printed && continue_line) {
		apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT(" "));
	}
	dt->line_started = 1;
}

static void ser_yaml_start_indent(struct serialize_yaml *dt, uint8_t flags)
{
	assert(dt->nest < ARRAY_SIZE(dt->flags));
	if (dt->nest > 0) dt->indent++;
	dt->flags[++dt->nest] = flags;
}

static void ser_yaml_newline(struct serialize_yaml *dt)
{
	apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT("\n"));
	dt->line_started = 0;
	dt->key_printed = 0;
}

static int ser_yaml_start_object(struct apk_serializer *ser, uint32_t schema_id)
{
	struct serialize_yaml *dt = container_of(ser, struct serialize_yaml, ser);

	ser_yaml_indent(dt, true, false);
	ser_yaml_start_indent(dt, 0);
	if (schema_id) {
		apk_ostream_fmt(dt->ser.os, "#%%SCHEMA: %08X", schema_id);
		ser_yaml_newline(dt);
	}
	return 0;
}

static int ser_yaml_start_array(struct apk_serializer *ser, int num)
{
	struct serialize_yaml *dt = container_of(ser, struct serialize_yaml, ser);

	if (num >= 0) {
		ser_yaml_indent(dt, true, true);
		apk_ostream_fmt(dt->ser.os, "# %d items", num);
	}
	ser_yaml_newline(dt);
	ser_yaml_start_indent(dt, F_ARRAY);
	return 0;
}

static int ser_yaml_end(struct apk_serializer *ser)
{
	struct serialize_yaml *dt = container_of(ser, struct serialize_yaml, ser);

	if (dt->line_started) {
		ser_yaml_indent(dt, false, true);
		apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT("# empty object"));
		ser_yaml_newline(dt);
	}
	dt->nest--;
	if (dt->nest) dt->indent--;
	return 0;
}

static int ser_yaml_comment(struct apk_serializer *ser, apk_blob_t comment)
{
	struct serialize_yaml *dt = container_of(ser, struct serialize_yaml, ser);

	ser_yaml_indent(dt, false, true);
	apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT("# "));
	apk_ostream_write_blob(dt->ser.os, comment);
	ser_yaml_newline(dt);
	return 0;
}

static int ser_yaml_key(struct apk_serializer *ser, apk_blob_t key)
{
	struct serialize_yaml *dt = container_of(ser, struct serialize_yaml, ser);

	if (dt->key_printed) ser_yaml_newline(dt);
	ser_yaml_indent(dt, true, true);
	apk_ostream_write_blob(dt->ser.os, key);
	apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT(":"));
	dt->key_printed = 1;
	return 0;
}

enum {
	QUOTE_NONE,
	QUOTE_SINGLE,
	QUOTE_BLOCK,
};

static int need_quoting(apk_blob_t b, int multiline)
{
	int style = QUOTE_NONE;

	if (!b.len) return QUOTE_NONE;
	if (b.len >= 80 || multiline) return QUOTE_BLOCK;

	// must not start with indicator character
	if (strchr("-?:,[]{}#&*!|>'\"%@`", b.ptr[0])) style = QUOTE_SINGLE;
	// must not contain ": " or " #"
	for (int i = 0, prev = i; i < b.len; i++) {
		switch (b.ptr[i]) {
		case '\r':
		case '\n':
		case '\'':
			return QUOTE_BLOCK;
		case ' ':
			if (prev == ':') style = QUOTE_SINGLE;
			break;
		case '#':
			// The adbgen parser requires ' #' to be block quited currently
			if (prev == ' ') return QUOTE_BLOCK;
			style = QUOTE_SINGLE;
			break;
		}
		prev = b.ptr[i];
	}
	return style;
}

static int ser_yaml_string(struct apk_serializer *ser, apk_blob_t scalar, int multiline)
{
	struct serialize_yaml *dt = container_of(ser, struct serialize_yaml, ser);

	ser_yaml_indent(dt, true, true);
	switch (need_quoting(scalar, multiline)) {
	case QUOTE_NONE:
		apk_ostream_write_blob(dt->ser.os, scalar);
		ser_yaml_newline(dt);
		break;
	case QUOTE_SINGLE:
		apk_ostream_write(dt->ser.os, "'", 1);
		apk_ostream_write_blob(dt->ser.os, scalar);
		apk_ostream_write(dt->ser.os, "'", 1);
		ser_yaml_newline(dt);
		break;
	case QUOTE_BLOCK:
	default:
		/* long or multiline */
		apk_ostream_write_blob(dt->ser.os, APK_BLOB_STRLIT("|"));
		ser_yaml_newline(dt);
		dt->indent++;
		apk_blob_foreach_token(line, scalar, APK_BLOB_STR("\n")) {
			ser_yaml_indent(dt, false, true);
			apk_ostream_write_blob(dt->ser.os, line);
			ser_yaml_newline(dt);
		}
		dt->indent--;
		break;
	}
	return 0;
}

static int ser_yaml_numeric(struct apk_serializer *ser, uint64_t val, int hint)
{
	struct serialize_yaml *dt = container_of(ser, struct serialize_yaml, ser);
	char buf[64];

	ser_yaml_indent(dt, true, true);
	apk_ostream_write_blob(dt->ser.os, apk_ser_format_numeric(ser, buf, sizeof buf, val, hint));
	ser_yaml_newline(dt);
	return 0;
}

const struct apk_serializer_ops apk_serializer_yaml = {
	.context_size = sizeof(struct serialize_yaml),
	.start_object = ser_yaml_start_object,
	.start_array = ser_yaml_start_array,
	.end = ser_yaml_end,
	.comment = ser_yaml_comment,
	.key = ser_yaml_key,
	.string = ser_yaml_string,
	.numeric = ser_yaml_numeric,
};
