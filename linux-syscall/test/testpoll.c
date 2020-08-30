#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <poll.h>
#include <assert.h>
#include <time.h>
#include <string.h>

int main(int argc, char **argv)
{
    int i;
    int ret;
    int fd;
    unsigned char keys_val;
    struct pollfd fds[2];
    int pipefd[2];
    struct timespec ts;

    // test poll using pipe
    if (pipe(pipefd) == -1)
    {
        printf("pipe");
        exit(-1);
    }

    // test time out
    fds[0].fd = 0;
    fds[0].events = POLLIN;
    ret = poll(fds, 1, 1000);
    assert(ret == 0);

    fds[0].fd = pipefd[0];
    fds[0].events = POLLIN;
    fds[1].fd = pipefd[1];
    fds[1].events = POLLOUT;

    ret = poll(fds, 2, 5000);
    assert(ret == 1);
    assert(fds[1].revents == POLLOUT);

    write(pipefd[1], "test", strlen("test"));

    ts.tv_sec = 5;
    ts.tv_nsec = 0;

    ret = ppoll(fds, 2, &ts, NULL);
    assert(ret == 2);
    assert(fds[0].revents == POLLIN);

    close(pipefd[0]);
    close(pipefd[1]);
    return 0;
}