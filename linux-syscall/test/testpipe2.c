#include <sys/wait.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <string.h>

int main(int argc, char *argv[])
{
	int writefd, readfd;
	sscanf(argv[2], "%d", &writefd);
	sscanf(argv[1], "%d", &readfd);
	close(readfd);
	write(writefd, "hello pipe", strlen("hello pipe"));
	close(writefd);
	exit(0);
}
