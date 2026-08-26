#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>

int mknodat(int dirfd, const char *pathname, mode_t mode, dev_t dev)
{
	int ret = 0;
	int curdir_fd = open(".", O_DIRECTORY | O_CLOEXEC);
	if (curdir_fd < 0)
		return -1;

	if (fchdir(dirfd) < 0) {
		ret = -1;
		goto cleanup;
	}

	/* if mknod fails, fall through and restore the original dirfd */
	if (mknod(pathname, mode, dev) < 0) {
		ret = -1;
	}

	if (fchdir(curdir_fd) < 0) {
		ret = -1;
		goto cleanup;
	}

cleanup:
	close(curdir_fd);
	return ret;
}
