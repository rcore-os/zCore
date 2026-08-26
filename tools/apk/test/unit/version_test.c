#include "apk_test.h"
#include "apk_io.h"
#include "apk_version.h"

static bool version_test_one(apk_blob_t arg)
{
	apk_blob_t ver1, ver2, op, space = APK_BLOB_STRLIT(" "), binvert = APK_BLOB_STRLIT("!");
	bool ok = false, invert = false;

	// trim comments and trailing whitespace
	apk_blob_split(arg, APK_BLOB_STRLIT("#"), &arg, &op);
	arg = apk_blob_trim(arg);
	if (!arg.len) return true;

	// arguments are either:
	//   "version"		-> check validity
	//   "!version"		-> check invalid
	//   "ver1 op ver2"	-> check if that the comparison is true
	//   "ver1 !op ver2"	-> check if that the comparison is false
	if (apk_blob_split(arg, space, &ver1, &op) &&
	    apk_blob_split(op,  space, &op,   &ver2)) {
		invert = apk_blob_pull_blob_match(&op, binvert);
		ok = apk_version_match(ver1, apk_version_result_mask_blob(op), ver2);
	} else {
		ver1 = arg;
		invert = apk_blob_pull_blob_match(&ver1, binvert);
		ok = apk_version_validate(ver1);
	}
	if (invert) ok = !ok;
	if (!ok) printf("FAIL: " BLOB_FMT "\n", BLOB_PRINTF(arg));
	return ok;
}

APK_TEST(version_test) {
	int errors = 0;
	apk_blob_t l;
	struct apk_istream *is;

	is  = apk_istream_from_file(AT_FDCWD, "version.data");
	assert_ptr_ok(is);

	while (apk_istream_get_delim(is, APK_BLOB_STR("\n"), &l) == 0)
		errors += (version_test_one(l) == false);

	assert_int_equal(errors, 0);
	assert_int_equal(apk_istream_close(is), 0);
}
