#ifndef _XOPEN_SOURCE
#define _XOPEN_SOURCE 700
#endif
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <sys/types.h>
#include <sys/sem.h>
#include <sys/wait.h>
#include <unistd.h>
#include <assert.h>
#include <stdio.h>

static const char path[] = ".";
static const int id = 's';

#define T(f) assert((f) != -1)

static void dec()
{
	key_t k;
	int semid, semval;
	struct sembuf sops;

	T(k = ftok(path, id));
	T(semid = semget(k, 0, 0));

	/* test sem_op < 0 */
	sops.sem_num = 0;
	sops.sem_op = -1;
	sops.sem_flg = 0;
	T(semop(semid, &sops, 1));
	T(semval = semctl(semid, 0, GETVAL));
	assert(semval == 0);

	/* cleanup */
	T(semctl(semid, 0, IPC_RMID));
}

int main(void)
{
	int p;
	int status;

	dec();
	return 0;
}
