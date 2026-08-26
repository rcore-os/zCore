#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "apk_adb.h"
#include "apk_applet.h"
#include "apk_extract.h"

struct conv_ctx {
	struct apk_ctx *ac;
	struct adb_obj pkgs;
	struct adb dbi;
	struct apk_extract_ctx ectx;
};

static int convert_v2index(struct apk_extract_ctx *ectx, apk_blob_t *desc, struct apk_istream *is)
{
	struct conv_ctx *ctx = container_of(ectx, struct conv_ctx, ectx);
	struct adb_obj pkginfo;
	apk_blob_t token = APK_BLOB_STR("\n"), l;
	int i;

	adb_wo_alloca(&pkginfo, &schema_pkginfo, &ctx->dbi);

	while (apk_istream_get_delim(is, token, &l) == 0) {
		if (l.len < 2) {
			adb_wa_append_obj(&ctx->pkgs, &pkginfo);
			continue;
		}
		i = adb_pkg_field_index(l.ptr[0]);
		if (i > 0) adb_wo_pkginfo(&pkginfo, i, APK_BLOB_PTR_LEN(l.ptr+2, l.len-2));
	}
	return apk_istream_close(is);
}

static const struct apk_extract_ops extract_convndx = {
	.v2index = convert_v2index,
};

static int load_index(struct conv_ctx *ctx, struct apk_istream *is)
{
	if (IS_ERR(is)) return PTR_ERR(is);
	apk_extract_init(&ctx->ectx, ctx->ac, &extract_convndx);
	return apk_extract(&ctx->ectx, is);
}

static int conv_main(void *pctx, struct apk_ctx *ac, struct apk_string_array *args)
{
	struct conv_ctx *ctx = pctx;
	struct apk_trust *trust = apk_ctx_get_trust(ac);
	struct apk_out *out = &ac->out;
	struct adb_obj ndx;
	int r;

	ctx->ac = ac;
	adb_w_init_alloca(&ctx->dbi, ADB_SCHEMA_INDEX, 1000);
	adb_wo_alloca(&ndx, &schema_index, &ctx->dbi);
	adb_wo_alloca(&ctx->pkgs, &schema_pkginfo_array, &ctx->dbi);

	apk_array_foreach_item(arg, args) {
		r = load_index(ctx, apk_istream_from_url(arg, apk_ctx_since(ac, 0)));
		if (r) {
			apk_err(out, "%s: %s", arg, apk_error_str(r));
			goto err;
		}
		apk_notice(out, "%s: %u packages", arg, adb_ra_num(&ctx->pkgs));
	}

	adb_wo_obj(&ndx, ADBI_NDX_PACKAGES, &ctx->pkgs);
	adb_w_rootobj(&ndx);

	r = adb_c_create(
		adb_compress(apk_ostream_to_fd(STDOUT_FILENO), &ac->compspec),
		&ctx->dbi, trust);
err:
	adb_free(&ctx->dbi);

	return r;
}

static struct apk_applet apk_convndx = {
	.name = "convndx",
	.optgroup_generation = 1,
	.context_size = sizeof(struct conv_ctx),
	.main = conv_main,
};
APK_DEFINE_APPLET(apk_convndx);
