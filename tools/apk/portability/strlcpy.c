#include <stddef.h>
#include <string.h>

size_t strlcpy(char *dst, const char *src, size_t size)
{
	size_t ret = strlen(src), len;
	if (!size) return ret;
	len = ret;
	if (len >= size) len = size - 1;
	memcpy(dst, src, len);
	dst[len] = 0;
	return ret;
}
