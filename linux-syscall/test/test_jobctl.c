// Job-control smoke test: stop/continue via wait4(WUNTRACED) and kill(SIGCONT).
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static int fail(const char *msg)
{
	fprintf(stderr, "FAIL: %s (errno=%d %s)\n", msg, errno, strerror(errno));
	return 1;
}

int main(void)
{
	printf("jobctl: parent pgid=%d ppid=%d\n", getpgid(0), getppid());

	pid_t pid = fork();
	if (pid < 0)
		return fail("fork");
	if (pid == 0) {
		if (setsid() < 0)
			return fail("child setsid");
		printf("jobctl: child sid=%d pgid=%d\n", getsid(0), getpgid(0));
		raise(SIGSTOP);
		_exit(99);
	}

	int status = 0;
	pid_t w = waitpid(pid, &status, WUNTRACED);
	if (w < 0)
		return fail("waitpid WUNTRACED");
	if (!WIFSTOPPED(status))
		return fail("child not reported stopped");
	printf("jobctl: child stopped by signal %d\n", WSTOPSIG(status));

	if (kill(pid, SIGCONT) < 0)
		return fail("kill SIGCONT");

	// Drain continued notification if supported, then exit status.
	for (;;) {
		w = waitpid(pid, &status, WUNTRACED | WCONTINUED);
		if (w < 0) {
			if (errno == EINTR)
				continue;
			return fail("waitpid after SIGCONT");
		}
		if (WIFCONTINUED(status)) {
			printf("jobctl: child continued\n");
			continue;
		}
		if (WIFSTOPPED(status))
			continue;
		break;
	}
	if (!WIFEXITED(status) || WEXITSTATUS(status) != 99)
		return fail("unexpected final status");

	printf("jobctl: PASS\n");
	return 0;
}
