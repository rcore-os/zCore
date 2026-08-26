#include <stdio.h>
#include <unistd.h>
#include "apk_adb.h"
#include "apk_applet.h"
#include "apk_print.h"

#define ADBDUMP_OPTIONS(OPT) \
	OPT(OPT_ADBDUMP_format,		APK_OPT_ARG "format")

APK_OPTIONS(adbdump_options_desc, ADBDUMP_OPTIONS);

struct adbdump_ctx {
	const struct apk_serializer_ops *ser;
};

static int adbdump_parse_option(void *pctx, struct apk_ctx *ac, int opt, const char *optarg)
{
	struct adbdump_ctx *ctx = pctx;

	switch (opt) {
	case OPT_ADBDUMP_format:
		ctx->ser = apk_serializer_lookup(optarg, &apk_serializer_yaml);
		if (IS_ERR(ctx->ser)) return -EINVAL;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static int adbdump_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct adbdump_ctx *ctx = pctx;
	struct apk_out *out = &ac->out;
	int r;

	if (!ctx->ser) ctx->ser = &apk_serializer_yaml;
	apk_array_foreach_item(arg, args) {
		r = adb_walk_adb(
			adb_decompress(apk_istream_from_file_mmap(AT_FDCWD, arg), NULL),
			apk_ostream_to_fd(STDOUT_FILENO),
			ctx->ser, ac);
		if (r) {
			apk_err(out, "%s: %s", arg, apk_error_str(r));
			return r;
		}
	}

	return 0;
}

static struct apk_applet apk_adbdump = {
	.name = "adbdump",
	.context_size = sizeof(struct adbdump_ctx),
	.options_desc = adbdump_options_desc,
	.parse = adbdump_parse_option,
	.main = adbdump_main,
};
APK_DEFINE_APPLET(apk_adbdump);
