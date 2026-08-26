#include <sys/socket.h>
#include <fcntl.h>
#undef socket

int __portable_socket(int domain, int type, int protocol)
{
	int fd = socket(domain, type & ~(SOCK_CLOEXEC|SOCK_NONBLOCK), protocol);
	if (fd < 0) return fd;
	if (type & SOCK_CLOEXEC) fcntl(fd, F_SETFD, fcntl(fd, F_GETFD) | FD_CLOEXEC);
	if (type & SOCK_NONBLOCK) fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK);
	return fd;
}
