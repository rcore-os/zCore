#include <stdio.h>
#include <stdlib.h>
#include <sys/time.h>
#include <time.h>
#include <sys/types.h>
#include <unistd.h>
#include <assert.h>
#include <string.h>

int main(void)
{
    fd_set rfds, wfds;
    struct timeval tv;
    struct timespec ts;
    int retval;

    int pipefd[2];

    // test time out 1s
    FD_ZERO(&rfds);
    FD_SET(0, &rfds);
    tv.tv_sec = 1;
    tv.tv_usec = 0;
    assert(select(1, &rfds, NULL, NULL, &tv) == 0);
    assert(!FD_ISSET(0, &rfds));

    FD_ZERO(&wfds);
    FD_SET(1, &wfds);
    ts.tv_sec = 5;
    ts.tv_nsec = 0;
    assert(pselect(2, NULL, &wfds, NULL, &ts, NULL) == 1);
    assert(FD_ISSET(1, &wfds));

    if (pipe(pipefd) == -1)
    {
        exit(-1);
    }
    write(pipefd[1], "test", strlen("test"));

    FD_ZERO(&rfds);
    FD_SET(pipefd[0], &rfds);
    FD_ZERO(&wfds);
    FD_SET(pipefd[1], &wfds);
    tv.tv_sec = 0;
    tv.tv_usec = 0;
    retval = select(pipefd[0] + 1, &rfds, &wfds, NULL, &tv);
    assert(FD_ISSET(pipefd[0], &rfds));
    assert(FD_ISSET(pipefd[1], &wfds));

    assert(retval == 2);

    close(pipefd[0]);
    close(pipefd[1]);
    exit(EXIT_SUCCESS);
}
