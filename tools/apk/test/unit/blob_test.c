#include "apk_test.h"
#include "apk_blob.h"
#include "apk_balloc.h"
#include "apk_print.h"

APK_TEST(blob_foreach_word_test) {
	int ch = 'a';
	apk_blob_foreach_word(word, APK_BLOB_STRLIT("a b   c d e  ")) {
		assert_int_equal(word.ptr[0], ch);
		assert_int_equal(word.len, 1);
		ch++;
	}
	assert_int_equal(ch, 'f');
}

APK_TEST(blob_contains) {
	assert_int_equal(-1, apk_blob_contains(APK_BLOB_STRLIT(" foo "), APK_BLOB_STRLIT("bar")));
	assert_int_equal(0, apk_blob_contains(APK_BLOB_STRLIT("bar bar"), APK_BLOB_STRLIT("bar")));
	assert_int_equal(4, apk_blob_contains(APK_BLOB_STRLIT("bar foo"), APK_BLOB_STRLIT("foo")));
}

static void _assert_split(apk_blob_t b, apk_blob_t split, apk_blob_t el, apk_blob_t er, const char *const file, int lineno)
{
	apk_blob_t l, r;
	_assert_int_equal(1, apk_blob_split(b, split, &l, &r), file, lineno);
	_assert_blob_equal(l, el, file, lineno);
	_assert_blob_equal(r, er, file, lineno);
}
#define assert_split(b, split, el, er) _assert_split(b, split, el, er, __FILE__, __LINE__)

APK_TEST(blob_split) {
	apk_blob_t l, r, foo = APK_BLOB_STRLIT("foo"), bar = APK_BLOB_STRLIT("bar");

	assert_int_equal(0, apk_blob_split(APK_BLOB_STRLIT("bar bar"), APK_BLOB_STRLIT("foo"), &l, &r));
	assert_split(APK_BLOB_STRLIT("bar foo"), APK_BLOB_STRLIT(" "), bar, foo);
	assert_split(APK_BLOB_STRLIT("bar = foo"), APK_BLOB_STRLIT(" = "), bar, foo);
}

APK_TEST(blob_url_sanitize) {
	struct {
		const char *url, *sanitized;
	} tests[] = {
		{ "http://example.com", NULL },
		{ "http://foo@example.com", NULL },
		{ "http://foo:pass@example.com", "http://foo:*@example.com" },
		{ "http://example.com/foo:pass@bar", NULL },
	};
	struct apk_balloc ba;
	apk_balloc_init(&ba, 64*1024);
	for (int i = 0; i < ARRAY_SIZE(tests); i++) {
		apk_blob_t url = APK_BLOB_STR(tests[i].url);
		apk_blob_t res = apk_url_sanitize(APK_BLOB_STR(tests[i].url), &ba);
		if (tests[i].sanitized) assert_blob_equal(APK_BLOB_STR(tests[i].sanitized), res);
		else assert_blob_identical(url, res);
	}
	apk_balloc_destroy(&ba);
}

APK_TEST(url_local) {
	assert_non_null(apk_url_local_file("/path/to/file", PATH_MAX));
	assert_non_null(apk_url_local_file("file:/path/to/file", PATH_MAX));
	assert_non_null(apk_url_local_file("file://localfile/path/to/file", PATH_MAX));
	assert_non_null(apk_url_local_file("test:/path/to/file", PATH_MAX));
	assert_non_null(apk_url_local_file("test_file://past-eos", 8));
	assert_null(apk_url_local_file("http://example.com", PATH_MAX));
	assert_null(apk_url_local_file("https://example.com", PATH_MAX));
	assert_null(apk_url_local_file("unknown://example.com", PATH_MAX));
}
