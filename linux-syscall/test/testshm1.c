#ifndef _XOPEN_SOURCE
#define _XOPEN_SOURCE 700
#endif
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <sys/shm.h>
#include <sys/stat.h>
#include <unistd.h>
#include <assert.h>
#include <stdio.h>

static const char path[] = ".";
static const int id = 'h';

#define T(f) assert((f)+1 != 0)
#define EQ(a,b) assert((a) == (b))

static void set()
{
	time_t t;
	key_t k;
	int shmid;
	struct shmid_ds shmid_ds;
	void *p;
	
	T(t = time(0));
	T(k = ftok(path, id));
	
	/* make sure we get a clean shared memory id */
	T(shmid = shmget(k, 100, IPC_CREAT|0666));
	T(shmctl(shmid, IPC_RMID, 0));
	T(shmid = shmget(k, 100, IPC_CREAT|IPC_EXCL|0666));

	/* check IPC_EXCL */
	errno = 0;
	if (shmget(k, 100, IPC_CREAT|IPC_EXCL|0666) != -1 || errno != EEXIST)
		printf("shmget(IPC_CREAT|IPC_EXCL) should have failed with EEXIST, got %s\n", strerror(errno));

	/* check if shmget initilaized the msshmid_ds structure correctly */
	
	T(shmctl(shmid, IPC_STAT, &shmid_ds));
	EQ(shmid_ds.shm_perm.mode & 0x1ff, 0666);
	EQ(shmid_ds.shm_segsz, 100);
	EQ(shmid_ds.shm_lpid, 0);
	EQ(shmid_ds.shm_cpid, getpid());
	EQ((int)shmid_ds.shm_nattch, 0);
	EQ((long long)shmid_ds.shm_atime, 0);
	EQ((long long)shmid_ds.shm_dtime, 0);
	if (shmid_ds.shm_ctime < t)
		printf("shmid_ds.shm_ctime >= t failed: got %lld, want >= %lld\n", (long long)shmid_ds.shm_ctime, (long long)t);
	if (shmid_ds.shm_ctime > t+5)
		printf("shmid_ds.shm_ctime <= t+5 failed: got %lld, want <= %lld\n", (long long)shmid_ds.shm_ctime, (long long)t+5);
	
	/* test attach */
	if ((p=shmat(shmid, 0, 0)) == 0)
		printf("shmat failed: %s\n", strerror(errno));
	
	T(shmctl(shmid, IPC_STAT, &shmid_ds));
	EQ((int)shmid_ds.shm_nattch, 1);
	EQ(shmid_ds.shm_lpid, getpid());
	if (shmid_ds.shm_atime < t)
		printf("shm_atime is %lld want >= %lld\n", (long long)shmid_ds.shm_atime, (long long)t);
	if (shmid_ds.shm_atime > t+5)
		printf("shm_atime is %lld want <= %lld\n", (long long)shmid_ds.shm_atime, (long long)t+5);
	
	strcpy((char *)p, "test data");
	T(shmdt(p));
}

int main(void)
{
	int p;
	int status;
	set();
	int pid = vfork();
	if (pid < 0)
        printf("error in fork!\n");
    else if (pid == 0)
    {
        execl("/bin/testshm2", "/bin/testshm2", NULL);
        exit(0);
    }

	return 0;
}
