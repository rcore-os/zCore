#include <fcntl.h>
#include <unistd.h>

int pipe2(int pipefd[2], int flags)
{
	int r;

	if ((r = pipe(pipefd)) < 0)
		return r;

	if (flags & O_CLOEXEC) {
		(void) fcntl(pipefd[0], F_SETFD, FD_CLOEXEC);
		(void) fcntl(pipefd[1], F_SETFD, FD_CLOEXEC);
	}

	if (flags & O_NONBLOCK) {
		(void) fcntl(pipefd[0], F_SETFL, O_NONBLOCK);
		(void) fcntl(pipefd[1], F_SETFL, O_NONBLOCK);
	}

	return 0;
}
