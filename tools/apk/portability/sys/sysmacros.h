#ifdef __linux__
# include_next <sys/sysmacros.h>
#else
# include <stdint.h>
# include <sys/types.h>
# define major(x)        ((int32_t)(((u_int32_t)(x) >> 24) & 0xff))
# define minor(x)        ((int32_t)((x) & 0xffffff))
# define makedev(x, y)    ((dev_t)(((x) << 24) | (y)))
#endif
