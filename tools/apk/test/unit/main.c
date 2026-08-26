#include <dlfcn.h>
#include <stdio.h>
#include <signal.h>
#include <unistd.h>
#include "apk_test.h"

static int num_tests;
static struct CMUnitTest all_tests[1000];

typeof(openat)* next_openat;
typeof(dup)* next_dup;

void test_register(const char *name, UnitTestFunction f)
{
	all_tests[num_tests++] = (struct CMUnitTest) {
		.name = name,
		.test_func = f,
	};
}

void test_out_open(struct test_out *to)
{
	to->out = (struct apk_out) {
		.out = fmemopen(to->buf_out, sizeof to->buf_out, "w"),
		.err = fmemopen(to->buf_err, sizeof to->buf_err, "w"),
	};
	assert_non_null(to->out.out);
	assert_non_null(to->out.err);
}

void assert_output_equal(struct test_out *to, const char *expected_err, const char *expected_out)
{
	fputc(0, to->out.out);
	fclose(to->out.out);
	fputc(0, to->out.err);
	fclose(to->out.err);

	assert_string_equal(to->buf_err, expected_err);
	assert_string_equal(to->buf_out, expected_out);
}

static void init_next_funcs(void)
{
	next_openat = dlsym(RTLD_NEXT, "openat");
	next_dup = dlsym(RTLD_NEXT, "dup");
}

int main(void)
{
	init_next_funcs();
	if (access("test/unit", F_OK) == 0) chdir("test/unit");
	signal(SIGPIPE, SIG_IGN);
	return _cmocka_run_group_tests("unit_tests", all_tests, num_tests, NULL, NULL);
}
