#include_next <sys/socket.h>

#ifdef NEED_SOCK_CLOEXEC
#define SOCK_CLOEXEC   02000000
#define SOCK_NONBLOCK  04000

int __portable_socket(int domain, int type, int protocol);
#define socket(...) __portable_socket(__VA_ARGS__)
#endif
