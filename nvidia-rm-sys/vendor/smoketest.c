/*
 * Bring-up smoke test only -- NOT vendored NVIDIA source. See build.rs for
 * why this file exists. To be deleted once real vendoring starts.
 *
 * Exercises the same call shape NVIDIA's RM uses against os-interface.h:
 * C code calling into a function implemented on the Rust side.
 */

/* Implemented in src/lib.rs as `extern "C" fn nvrm_smoketest_log`. */
extern void nvrm_smoketest_log(unsigned int value);

unsigned int nvrm_smoketest_add(unsigned int a, unsigned int b) {
    unsigned int sum = a + b;
    nvrm_smoketest_log(sum);
    return sum;
}
