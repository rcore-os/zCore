#include <time.h>
#include <stdio.h>
#include <unistd.h>
#include <assert.h>
#include <sys/time.h>
#include <sys/times.h>
#include <sys/types.h>
#include <sys/resource.h>

int main(int argc, char **argv)
{
    struct timespec ts = {0, 0};
    clock_gettime(CLOCK_REALTIME, &ts);
    printf("timespec: %ld sec, %ld nsec\n", ts.tv_sec, ts.tv_nsec);
    assert(ts.tv_sec != 0 && ts.tv_nsec != 0);

    struct timeval tv;

    // the musl-libc call clock_gettime instead..qwq
    gettimeofday(&tv, NULL);
    printf("timeval: %ld sec, %ld usec\n", tv.tv_sec, tv.tv_usec);
    assert(tv.tv_sec != 0 && tv.tv_usec != 0);

    // the musl-libc call clock_gettime instead..qwq
    time_t seconds;
    seconds = time(NULL);
    printf("time: %ld\n", seconds);
    assert(seconds != 0);

    struct tms tmp;
    clock_t t = times(&tmp);
    printf("times return: %ld\n", t);
    assert(times != 0);

    struct rusage usage;
    getrusage(0, &usage);
    printf("timeval getrusage user: %ld sec, %ld usec\n", usage.ru_utime.tv_sec, usage.ru_utime.tv_usec);
    printf("timeval getrusage system: %ld sec, %ld usec\n", usage.ru_stime.tv_sec, usage.ru_stime.tv_usec);
    assert(usage.ru_utime.tv_sec != 0 && usage.ru_utime.tv_usec != 0);

    return 0;
}