#include <stdlib.h>
#include <unistd.h>

#include "apk_test.h"
#include "apk_print.h"
#include "apk_process.h"
#include "apk_io.h"

#define writestr(fd, str) write(fd, str, sizeof(str)-1)

APK_TEST(pid_logging) {
	struct test_out to;
	struct apk_process p;

	test_out_open(&to);
	assert_int_equal(0, apk_process_init(&p, "test0", "test0: ", &to.out, NULL));
	if (apk_process_fork(&p) == 0) {
		writestr(STDERR_FILENO, "error1\nerror2\n");
		writestr(STDOUT_FILENO, "hello1\nhello2\n");
		close(STDOUT_FILENO);
		usleep(10000);
		writestr(STDERR_FILENO, "more\nlastline");
		exit(0);
	}

	assert_int_equal(0, apk_process_run(&p));
	assert_output_equal(&to,
		"test0: error1\n"
		"test0: error2\n"
		"test0: more\n"
		"test0: lastline\n",

		"test0: hello1\n"
		"test0: hello2\n");
}

APK_TEST(pid_error_exit) {
	struct test_out to;
	struct apk_process p;

	test_out_open(&to);
	assert_int_equal(0, apk_process_init(&p, "test1", "test1: ", &to.out, NULL));
	if (apk_process_fork(&p) == 0) {
		exit(100);
	}

	assert_int_equal(-1, apk_process_run(&p));
	assert_output_equal(&to,
		"ERROR: test1: exited with error 100\n",
		"");
}

APK_TEST(pid_input_partial) {
	struct test_out to;
	struct apk_process p;

	test_out_open(&to);
	assert_int_equal(0, apk_process_init(&p, "test2", "test2: ", &to.out, apk_istream_from_file(AT_FDCWD, "/dev/zero")));
	if (apk_process_fork(&p) == 0) {
		char buf[1024];
		int left = 128*1024;
		while (left) {
			int n = read(STDIN_FILENO, buf, min(left, sizeof buf));
			if (n <= 0) exit(100);
			left -= n;
		}
		writestr(STDOUT_FILENO, "success\n");
		exit(0);
	}

	assert_int_equal(-2, apk_process_run(&p));
	assert_output_equal(&to,
		"",
		"test2: success\n");
}

APK_TEST(pid_input_full) {
	struct test_out to;
	struct apk_process p;

	test_out_open(&to);
	assert_int_equal(0, apk_process_init(&p, "test3", "test3: ", &to.out, apk_istream_from_file(AT_FDCWD, "version.data")));
	if (apk_process_fork(&p) == 0) {
		char buf[1024];
		writestr(STDOUT_FILENO, "start reading!\n");
		usleep(10000);
		while (1) {
			int n = read(STDIN_FILENO, buf, sizeof buf);
			if (n < 0) exit(100);
			if (n == 0) break;
		}
		writestr(STDOUT_FILENO, "success\n");
		exit(0);
	}

	assert_int_equal(0, apk_process_run(&p));
	assert_output_equal(&to,
		"",
		"test3: start reading!\n"
		"test3: success\n");
}

static void test_process_istream(int rc, char *arg, const char *expect_err, const char *expect_out)
{
	struct test_out to;
	char out[256], *argv[] = { "../process-istream.sh", arg, NULL };

	test_out_open(&to);
	struct apk_istream *is = apk_process_istream(argv, &to.out, "process-istream: ");
	assert_ptr_ok(is);

	int n = apk_istream_read_max(is, out, sizeof out);
	assert_int_equal(rc, apk_istream_close(is));

	assert_output_equal(&to, expect_err, "");
	assert_int_equal(strlen(expect_out), n);
	assert_memory_equal(expect_out, out, n);
}

APK_TEST(pid_istream_ok) {
	test_process_istream(0, "ok",
		"process-istream: stderr text\n"
		"process-istream: stderr again\n",
		"hello\nhello again\n");
}

APK_TEST(pid_istream_fail) {
	test_process_istream(-APKE_REMOTE_IO, "fail",
		"process-istream: stderr text\n"
		"ERROR: process-istream.sh: exited with error 10\n",
		"hello\n");
}
