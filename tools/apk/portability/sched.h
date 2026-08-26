#include_next <sched.h>

#ifdef NEED_UNSHARE
# define unshare(flags) ({errno = ENOSYS; -1;})
#endif
