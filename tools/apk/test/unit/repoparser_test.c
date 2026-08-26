#include "apk_test.h"
#include "apk_repoparser.h"

static int test_repository(struct apk_repoparser *rp, apk_blob_t url, const char *index_file, apk_blob_t tag)
{
	apk_out(rp->out, BLOB_FMT ":%s:" BLOB_FMT, BLOB_PRINTF(url), index_file ?: "", BLOB_PRINTF(tag));
	return 0;
}

static const struct apk_repoparser_ops ops = {
	.repository = test_repository,
};

static void repo_test(bool allow_keywords, const char *data, const char *expect_stderr, const char *expect_stdout)
{
	struct test_out to;
	struct apk_repoparser rp;

	test_out_open(&to);
	apk_repoparser_init(&rp, &to.out, &ops);
	apk_repoparser_set_file(&rp, "repositories");
	apk_blob_foreach_token(line, APK_BLOB_STR(data), APK_BLOB_STRLIT("\n"))
		apk_repoparser_parse(&rp, line, allow_keywords);
	assert_output_equal(&to, expect_stderr, expect_stdout);
	apk_repoparser_free(&rp);
}

APK_TEST(repoparser_basic) {
	repo_test(true,
		"# test data\n"
		"http://example.com/edge/main\n"
		"@tag http://example.com/edge/testing\n"
		"ndx http://example.com/repo/Packages.adb\n"
		"v2 http://example.com/main\n"
		"v3 http://example.com/main\n"
		"v3 @tag http://example.com/testing\n",
		"",
		"http://example.com/edge/main:APKINDEX.tar.gz:\n"
		"http://example.com/edge/testing:APKINDEX.tar.gz:@tag\n"
		"http://example.com/repo/Packages.adb::\n"
		"http://example.com/main:APKINDEX.tar.gz:\n"
		"http://example.com/main:Packages.adb:\n"
		"http://example.com/testing:Packages.adb:@tag\n");
}

APK_TEST(repoparser_components) {
	repo_test(true,
		"http://example.com/ main community\n"
		"v3 @tag http://example.com main community\n"
		"foo http://example.com/alpine/testing\n",
		"WARNING: repositories:3: unrecogized keyword: foo\n",
		"http://example.com/main:APKINDEX.tar.gz:\n"
		"http://example.com/community:APKINDEX.tar.gz:\n"
		"http://example.com/main:Packages.adb:@tag\n"
		"http://example.com/community:Packages.adb:@tag\n");
}

APK_TEST(repoparser_variables) {
	repo_test(true,
		"set -unknown mirror=alpine.org\n"
		"set -default mirror=alpine.org\n"
		"http://${mirror}/main\n"
		"set mirror=example.com\n"
		"http://${mirror}/main\n"
		"set -default mirror=alpine.org\n"
		"http://${mirror}/main\n"
		"http://${undefined}/main\n"
		"set mirror=${mirror}/alpine\n"
		"set comp=main community testing\n"
		"set var-foo=bad-name\n"
		"set APK_FOO=reserved\n"
		"http://${mirror}/ ${comp}\n"
		"v2 foobar main\n",
		"WARNING: repositories:1: invalid option: -unknown\n"
		"WARNING: repositories:8: undefined variable: undefined\n"
		"WARNING: repositories:11: invalid variable definition: var-foo=bad-name\n"
		"WARNING: repositories:12: invalid variable definition: APK_FOO=reserved\n"
		"WARNING: repositories:14: invalid url: foobar\n",
		"http://alpine.org/main:APKINDEX.tar.gz:\n"
		"http://example.com/main:APKINDEX.tar.gz:\n"
		"http://example.com/main:APKINDEX.tar.gz:\n"
		"http://example.com/alpine/main:APKINDEX.tar.gz:\n"
		"http://example.com/alpine/community:APKINDEX.tar.gz:\n"
		"http://example.com/alpine/testing:APKINDEX.tar.gz:\n"
		);
}

APK_TEST(repoparser_nokeywords) {
	repo_test(false,
		"set mirror=alpine.org\n"
		"repository\n"
		"http://www.alpinelinux.org/main\n",
		"",
		"set/mirror=alpine.org:APKINDEX.tar.gz:\n"
		"repository:APKINDEX.tar.gz:\n"
		"http://www.alpinelinux.org/main:APKINDEX.tar.gz:\n"
		);
}
