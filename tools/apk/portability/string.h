#include_next <string.h>

#ifdef NEED_MEMRCHR
void *memrchr(const void *m, int c, size_t n);
#endif

#ifdef NEED_STRCHRNUL
char *strchrnul(const char *s, int c);
#endif

#ifdef NEED_STRLCPY
size_t strlcpy(char *dst, const char *src, size_t size);
#endif
