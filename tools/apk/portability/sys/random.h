#ifdef NEED_GETRANDOM
#include <sys/types.h>

ssize_t getrandom(void *buf, size_t buflen, unsigned int flags);
#else
#include_next <sys/random.h>
#endif
