#include <string.h>

char *strchrnul(const char *s, int c)
{
	return strchr(s, c) ?: (char *)s + strlen(s);
}
