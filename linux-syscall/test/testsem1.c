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
#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>
#include <assert.h>
#include <stdio.h>

static const char path[] = ".";
static const int id = 's';

#define T(f) assert((f) != -1)

static void inc()
{
	time_t t;
	key_t k;
	int semid, semval, sempid, semncnt, semzcnt;
	struct semid_ds semid_ds;
	union semun {
		int val;
		struct semid_ds *buf;
		unsigned short *array;
	} arg;
	struct sembuf sops;

	T(t = time(0));
	T(k = ftok(path, id));

	/* make sure we get a clean semaphore id */
	T(semid = semget(k, 1, IPC_CREAT | 0666));
	T(semctl(semid, 0, IPC_RMID));
	T(semid = semget(k, 1, IPC_CREAT | IPC_EXCL | 0666));

	/* check IPC_EXCL */
	errno = 0;
	if (semget(k, 1, IPC_CREAT | IPC_EXCL | 0666) != -1 || errno != EEXIST)
		printf("semget(IPC_CREAT|IPC_EXCL) should have failed with EEXIST, got %s\n", strerror(errno));

	/* check if msgget initilaized the msqid_ds structure correctly */
	arg.buf = &semid_ds;
	T(semctl(semid, 0, IPC_STAT, arg));
	if (semid_ds.sem_ctime < t)
		printf("semid_ds.sem_ctime >= t failed: got %lld, want >= %lld\n", (long long)semid_ds.sem_ctime, (long long)t);
	if (semid_ds.sem_ctime > t + 5)
		printf("semid_ds.sem_ctime <= t+5 failed: got %lld, want <= %lld\n", (long long)semid_ds.sem_ctime, (long long)t + 5);

	/* test sem_op > 0 */
	sops.sem_num = 0;
	sops.sem_op = 1;
	sops.sem_flg = 0;
	T(semval = semctl(semid, 0, GETVAL));
	assert(semval == 0);
	T(semop(semid, &sops, 1));
	T(semval = semctl(semid, 0, GETVAL));
	assert(semval == 1);
	T(sempid = semctl(semid, 0, GETPID));
	assert(sempid == getpid());
	T(semncnt = semctl(semid, 0, GETNCNT));
	assert(semncnt == 0);
	T(semzcnt = semctl(semid, 0, GETZCNT));
	assert(semzcnt == 0);
}

int main(void)
{
	int p;
	int status;
	inc();
	int pid = vfork();
	if (pid < 0)
        printf("error in fork!\n");
    else if (pid == 0)
    {
        execl("/bin/testsem2", "/bin/testsem2", NULL);
        exit(0);
    }
	return 0;
}
