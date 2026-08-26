#include <sys/random.h>
#include <sys/types.h>
#include <unistd.h>
#include <fcntl.h>

ssize_t getrandom(void *buf, size_t buflen, unsigned int flags)
{
	int fd;
	ssize_t ret;

	fd = open("/dev/urandom", O_RDONLY|O_CLOEXEC);
	if (fd < 0)
		return -1;

	ret = read(fd, buf, buflen);
	close(fd);
	return ret;
}

