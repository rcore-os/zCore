#ifndef _XOPEN_SOURCE
#define _XOPEN_SOURCE 700
#endif
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <sys/types.h>
#include <sys/shm.h>
#include <sys/stat.h>
#include <unistd.h>
#include <assert.h>
#include <stdio.h>

static const char path[] = ".";
static const int id = 'h';

#define T(f) assert((f)+1 != 0)
#define EQ(a,b) assert((a) == (b))

static void get()
{
	key_t k;
	int shmid;
	void *p;

	T(k = ftok(path, id));
	T(shmid = shmget(k, 0, 0));

	errno = 0;
	if ((p=shmat(shmid, 0, SHM_RDONLY)) == 0)
		printf("shmat failed: %s\n", strerror(errno));

	if (strcmp((char *)p, "test data") != 0)
		printf("reading shared mem failed: got \"%.100s\" want \"test data\"\n", (char *)p);

	/* cleanup */
	T(shmdt(p));
	T(shmctl(shmid, IPC_RMID, 0));
}

int main(void)
{
	int p;
	int status;

	get();

	return 0;
}
