#include_next <sys/stat.h>

#ifdef NEED_MKNODAT
int mknodat(int dirfd, const char *pathname, mode_t mode, dev_t dev);
#endif
