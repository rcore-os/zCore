#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "adb.h"
#include "apk_applet.h"
#include "apk_print.h"

struct sign_ctx {
	struct apk_ctx *ac;

	struct adb db;
	struct apk_istream *is;
	struct apk_ostream *os;
	struct adb_verify_ctx vfy;

	unsigned int reset_signatures : 1;
	unsigned int signatures_written : 1;
};

#define ADBSIGN_OPTIONS(OPT) \
	OPT(OPT_ADBSIGN_reset_signatures,	"reset-signatures")

APK_OPTIONS(adbsign_options_desc, ADBSIGN_OPTIONS);

static int adbsign_parse_option(void *pctx, struct apk_ctx *ac, int optch, const char *optarg)
{
	struct sign_ctx *ctx = (struct sign_ctx *) pctx;

	switch (optch) {
	case OPT_ADBSIGN_reset_signatures:
		ctx->reset_signatures = 1;
		break;
	default:
		return -ENOTSUP;
	}
	return 0;
}

static int process_signatures(struct sign_ctx *ctx)
{
	int r;

	if (ctx->signatures_written) return 0;
	ctx->signatures_written = 1;
	r = adb_trust_write_signatures(apk_ctx_get_trust(ctx->ac), &ctx->db, &ctx->vfy, ctx->os);
	if (r < 0) apk_ostream_cancel(ctx->os, r);
	return r;
}

static int process_block(struct adb *db, struct adb_block *blk, struct apk_istream *is)
{
	struct sign_ctx *ctx = container_of(db, struct sign_ctx, db);
	int r;

	switch (adb_block_type(blk)) {
	case ADB_BLOCK_ADB:
		adb_c_header(ctx->os, db);
		return adb_c_block_copy(ctx->os, blk, is, &ctx->vfy);
	case ADB_BLOCK_SIG:
		if (ctx->reset_signatures)
			break;
		return adb_c_block_copy(ctx->os, blk, is, NULL);
	default:
		r = process_signatures(ctx);
		if (r < 0) return r;
		return adb_c_block_copy(ctx->os, blk, is, NULL);
	}
	return 0;
}

static int adbsign_resign(struct sign_ctx *ctx, struct apk_istream *is, struct apk_ostream *os)
{
	int r;

	if (IS_ERR(os)) {
		apk_istream_close(is);
		return PTR_ERR(os);
	}
	ctx->os = os;
	memset(&ctx->vfy, 0, sizeof ctx->vfy);
	r = adb_m_process(&ctx->db, is, 0, &ctx->ac->trust, NULL, process_block);
	if (r == 0) r = process_signatures(ctx);
	adb_free(&ctx->db);
	return apk_ostream_close_error(os, r);
}

static int adbsign_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct apk_out *out = &ac->out;
	struct sign_ctx *ctx = pctx;
	struct adb_compression_spec spec;
	int r;

	ctx->ac = ac;
	apk_array_foreach_item(arg, args) {
		struct apk_istream *is = adb_decompress(apk_istream_from_file_mmap(AT_FDCWD, arg), &spec);
		if (ac->compspec.alg || ac->compspec.level) spec = ac->compspec;
		struct apk_ostream *os = adb_compress(apk_ostream_to_file(AT_FDCWD, arg, 0644), &spec);
		r = adbsign_resign(ctx, is, os);
		if (r) apk_err(out, "%s: %s", arg, apk_error_str(r));
	}

	return 0;
}

static struct apk_applet apk_adbsign = {
	.name = "adbsign",
	.context_size = sizeof(struct sign_ctx),
	.options_desc = adbsign_options_desc,
	.optgroup_generation = 1,
	.parse = adbsign_parse_option,
	.main = adbsign_main,
};

APK_DEFINE_APPLET(apk_adbsign);
