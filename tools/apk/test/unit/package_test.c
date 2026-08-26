#include "apk_test.h"
#include "apk_database.h"
#include "apk_package.h"
#include "apk_blob.h"

APK_TEST(blob_subst) {
	struct apk_name *name = alloca(sizeof(struct apk_name) + 5);
	struct apk_package *pkg = alloca(sizeof(struct apk_package) + APK_DIGEST_LENGTH_SHA1);
	char buf[1024];

	*name = (struct apk_name) {};
	memcpy(name->name, "test", 5);
	*pkg = (struct apk_package) {
		.name = name,
		.version = &APK_BLOB_STRLIT("1.0-r0"),
		.arch = &APK_BLOB_STRLIT("noarch"),
		.digest_alg = APK_DIGEST_SHA1,
	};
	memcpy(pkg->digest, (uint8_t []) {
		0x12, 0x34, 0xab, 0xcd, 0xef, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
		0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00,
	}, 20);

	assert_int_equal(11, apk_blob_subst(buf, sizeof buf, APK_BLOB_STRLIT("${name}-${version}"), apk_pkg_subst, pkg));
	assert_string_equal(buf, "test-1.0-r0");

	assert_int_equal(11, apk_blob_subst(buf, sizeof buf, APK_BLOB_STRLIT("${name}-${arch}"), apk_pkg_subst, pkg));
	assert_string_equal(buf, "test-noarch");

	assert_int_equal(17, apk_blob_subst(buf, sizeof buf, APK_BLOB_STRLIT("${name}.${hash:8}.apk"), apk_pkg_subst, pkg));
	assert_string_equal(buf, "test.1234abcd.apk");

	assert_int_equal(19, apk_blob_subst(buf, sizeof buf, APK_BLOB_STRLIT("${name:3}/${name}-${version}.apk"), apk_pkg_subst, pkg));
	assert_string_equal(buf, "tes/test-1.0-r0.apk");

	assert_int_equal(20, apk_blob_subst(buf, sizeof buf, APK_BLOB_STRLIT("${name:8}/${name}-${version}.apk"), apk_pkg_subst, pkg));
	assert_string_equal(buf, "test/test-1.0-r0.apk");

	assert_int_equal(apk_blob_subst(buf, sizeof buf, APK_BLOB_STRLIT("${invalid}"), apk_pkg_subst, pkg), -APKE_PACKAGE_NAME_SPEC);
	assert_int_equal(apk_blob_subst(buf, sizeof buf, APK_BLOB_STRLIT("${hash:8s}"), apk_pkg_subst, pkg), -APKE_FORMAT_INVALID);
}

APK_TEST(pkg_subst_validate) {
	assert_int_equal(0, apk_pkg_subst_validate(APK_BLOB_STRLIT("${name}-${version}.apk")));
	assert_int_equal(0, apk_pkg_subst_validate(APK_BLOB_STRLIT("${name}-${version}.${hash:8}.apk")));
	assert_int_equal(0, apk_pkg_subst_validate(APK_BLOB_STRLIT("${name}_${version}_${arch}.apk")));
	assert_int_equal(0, apk_pkg_subst_validate(APK_BLOB_STRLIT("${arch}/${name}_${version}_${arch}.apk")));
	assert_int_equal(0, apk_pkg_subst_validate(APK_BLOB_STRLIT("${name:3}/${name}_${version}_${arch}.apk")));

	assert_int_equal(-APKE_PACKAGE_NAME_SPEC, apk_pkg_subst_validate(APK_BLOB_STRLIT("${arch}/${name}=${version}.apk")));
	assert_int_equal(-APKE_PACKAGE_NAME_SPEC, apk_pkg_subst_validate(APK_BLOB_STRLIT("${arch}_${name}_${version}.apk")));
}
