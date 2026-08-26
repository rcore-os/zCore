#pragma once
#include <unistd.h>
#ifdef __linux__
#include <sched.h>
#endif

static inline int apk_get_nproc(void)
{
#ifdef __linux__
	cpu_set_t cset;
	sched_getaffinity(0, sizeof(cset), &cset);
	return CPU_COUNT(&cset);
#else
	return (int)sysconf(_SC_NPROCESSORS_ONLN);
#endif
}
