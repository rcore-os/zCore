#include <limits.h>
#include <stdarg.h>
#include <stddef.h>
#include <setjmp.h>
#include <cmocka.h>
#include "apk_print.h"

#define assert_ptr_ok(c) _assert_true(!IS_ERR(c), #c, __FILE__, __LINE__)

#define _assert_blob_equal(a, b, file, line) do { \
		_assert_int_equal(a.len, b.len, file, line); \
		_assert_memory_equal(a.ptr, b.ptr, a.len, file, line); \
	} while (0)
#define assert_blob_equal(a, b) _assert_blob_equal(a, b, __FILE__, __LINE__)

#define _assert_blob_identical(a, b, file, line) do { \
		_assert_int_equal(a.len, b.len, file, line); \
		_assert_int_equal(cast_ptr_to_largest_integral_type(a.ptr), \
				  cast_ptr_to_largest_integral_type(b.ptr), \
				  file, line); \
	} while (0)
#define assert_blob_identical(a, b) _assert_blob_identical(a, b, __FILE__, __LINE__)

void test_register(const char *, UnitTestFunction);

#define APK_TEST(test_name) \
	static void test_name(void **); \
	__attribute__((constructor)) static void _test_register_##test_name(void) { test_register(#test_name, test_name); } \
	static void test_name(void **)

struct test_out {
	struct apk_out out;
	char buf_err[1024], buf_out[4*1024];
};

void test_out_open(struct test_out *to);
void assert_output_equal(struct test_out *to, const char *expected_err, const char *expected_out);
