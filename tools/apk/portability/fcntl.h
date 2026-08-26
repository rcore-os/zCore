#include_next <fcntl.h>

#ifndef F_SEAL_SEAL
#define F_SEAL_SEAL	0x0001
#endif
#ifndef F_SEAL_SHRINK
#define F_SEAL_SHRINK	0x0002
#endif
#ifndef F_SEAL_GROW
#define F_SEAL_GROW	0x0004
#endif
#ifndef F_SEAL_WRITE
#define F_SEAL_WRITE	0x0008
#endif
#ifndef F_SEAL_FUTURE_WRITE
#define F_SEAL_FUTURE_WRITE	0x0010
#endif
#ifndef F_SEAL_EXEC
#define F_SEAL_EXEC	0x0020
#endif
