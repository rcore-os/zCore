#pragma once
#include <unistd.h>
#include <sys/xattr.h>

static inline int apk_fsetxattr(int fd, const char *name, void *value, size_t size)
{
#ifdef __APPLE__
	return fsetxattr(fd, name, value, size, 0, 0);
#else
	return fsetxattr(fd, name, value, size, 0);
#endif
}

static inline ssize_t apk_fgetxattr(int fd, const char *name, void *value, size_t size)
{
#ifdef __APPLE__
	return fgetxattr(fd, name, value, size, 0, 0);
#else
	return fgetxattr(fd, name, value, size);
#endif
}

static inline ssize_t apk_flistxattr(int fd, char *namebuf, size_t size)
{
#ifdef __APPLE__
	return flistxattr(fd, namebuf, size, 0);
#else
	return flistxattr(fd, namebuf, size);
#endif
}
