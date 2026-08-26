/* pid.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2024 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <poll.h>
#include <errno.h>
#include <fcntl.h>
#include <spawn.h>
#include <unistd.h>
#include <sys/wait.h>

#include "apk_io.h"
#include "apk_process.h"
#include "apk_print.h"

#define APK_EXIT_STATUS_MAX_SIZE 128

static int apk_exit_status_str(int status, char *buf, size_t sz)
{
	if (WIFEXITED(status) && WEXITSTATUS(status) == 0)
		return 0;
	if (WIFEXITED(status))
		return apk_fmt(buf, sz, "exited with error %d", WEXITSTATUS(status));
	if (WIFSIGNALED(status))
		return apk_fmt(buf, sz, "killed by signal %d", WTERMSIG(status));
	if (WIFSTOPPED(status))
		return apk_fmt(buf, sz, "stopped by signal %d", WSTOPSIG(status));
	if (WIFCONTINUED(status))
		return apk_fmt(buf, sz, "continued");
	return apk_fmt(buf, sz, "status unknown %x", status);
}

static void close_fd(int *fd)
{
	if (*fd <= 0) return;
	close(*fd);
	*fd = -1;
}

static void set_non_blocking(int fd)
{
	if (fd >= 0) fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK);
}

int apk_process_init(struct apk_process *p, const char *argv0, const char *logpfx, struct apk_out *out, struct apk_istream *is)
{
	int ret;

	const char *linepfx = strrchr(logpfx, '\n');
	if (linepfx) linepfx++;
	else linepfx = logpfx;

	*p = (struct apk_process) {
		.logpfx = logpfx,
		.linepfx = linepfx,
		.argv0 = argv0,
		.is = is,
		.out = out,
	};
	if (IS_ERR(is)) return -PTR_ERR(is);

	ret = pipe2(p->pipe_stdin, O_CLOEXEC);
	if (ret < 0) return errno;

	if (!is) {
		close(p->pipe_stdin[1]);
		p->pipe_stdin[1] = -1;
	}

	ret = pipe2(p->pipe_stdout, O_CLOEXEC);
	if (ret < 0) {
		close(p->pipe_stdin[0]);
		close(p->pipe_stdin[1]);
		return errno;
	}
	ret = pipe2(p->pipe_stderr, O_CLOEXEC);
	if (ret < 0) {
		close(p->pipe_stdin[0]);
		close(p->pipe_stdin[1]);
		close(p->pipe_stdout[0]);
		close(p->pipe_stdout[1]);
		return errno;
	}

	set_non_blocking(p->pipe_stdin[1]);
	set_non_blocking(p->pipe_stdout[0]);
	set_non_blocking(p->pipe_stderr[0]);

	return 0;
}

// temporary sanitation to remove duplicate "* " prefix from a script output.
// remove when all package scripts are updated to accommodate apk prefixing output.
static uint8_t *sanitize_prefix(uint8_t *pos, uint8_t *end)
{
	switch (end - pos) {
	default:
		if (pos[0] != '*') return pos;
		if (pos[1] != ' ') return pos;
		return pos + 2;
	case 1:
		if (pos[0] != '*') return pos;
		return pos + 1;
	case 0:
		return pos;
	}
}

static int buf_process(struct buf *b, int fd, struct apk_out *out, const char *prefix, struct apk_process *p)
{
	ssize_t n = read(fd, &b->buf[b->len], sizeof b->buf - b->len);
	if (n > 0) b->len += n;

	uint8_t *pos, *lf, *end = &b->buf[b->len];
	for (pos = b->buf; (lf = memchr(pos, '\n', end - pos)) != NULL; pos = lf + 1) {
		pos = sanitize_prefix(pos, lf);
		apk_out_fmt(out, prefix, "%s%.*s", p->logpfx, (int)(lf - pos), pos);
		p->logpfx = p->linepfx;
	}
	if (n > 0) {
		b->len = end - pos;
		memmove(b->buf, pos, b->len);
		return 1;
	}
	if (pos != end) {
		pos = sanitize_prefix(pos, end);
		apk_out_fmt(out, prefix, "%s%.*s", p->logpfx, (int)(end - pos), pos);
	}
	return 0;
}

pid_t apk_process_fork(struct apk_process *p)
{
	pid_t pid = fork();
	if (pid < 0) return pid;
	if (pid == 0) {
		dup2(p->pipe_stdin[0], STDIN_FILENO);
		dup2(p->pipe_stdout[1], STDOUT_FILENO);
		dup2(p->pipe_stderr[1], STDERR_FILENO);
		close_fd(&p->pipe_stdin[1]);
		close_fd(&p->pipe_stdout[0]);
		close_fd(&p->pipe_stderr[0]);
		return pid;
	} else {
		p->pid = pid;
	}
	close_fd(&p->pipe_stdin[0]);
	close_fd(&p->pipe_stdout[1]);
	close_fd(&p->pipe_stderr[1]);
	return pid;
}

int apk_process_spawn(struct apk_process *p, const char *path, char * const* argv, char * const* env)
{
	posix_spawn_file_actions_t act;
	int r;

	posix_spawn_file_actions_init(&act);
	posix_spawn_file_actions_adddup2(&act, p->pipe_stdin[0], STDIN_FILENO);
	posix_spawn_file_actions_adddup2(&act, p->pipe_stdout[1], STDOUT_FILENO);
	posix_spawn_file_actions_adddup2(&act, p->pipe_stderr[1], STDERR_FILENO);
	r = posix_spawnp(&p->pid, path, &act, 0, argv, env ?: environ);
	posix_spawn_file_actions_destroy(&act);

	close_fd(&p->pipe_stdin[0]);
	close_fd(&p->pipe_stdout[1]);
	close_fd(&p->pipe_stderr[1]);
	return -r;
}

static int apk_process_handle(struct apk_process *p, bool break_on_stdout)
{
	struct pollfd fds[3] = {
		{ .fd = p->pipe_stdout[0], .events = POLLIN },
		{ .fd = p->pipe_stderr[0], .events = POLLIN },
		{ .fd = p->pipe_stdin[1],  .events = POLLOUT },
	};

	while (fds[0].fd >= 0 || fds[1].fd >= 0 || fds[2].fd >= 0) {
		if (poll(fds, ARRAY_SIZE(fds), -1) <= 0) continue;
		if (fds[0].revents && !break_on_stdout) {
			if (!buf_process(&p->buf_stdout, p->pipe_stdout[0], p->out, NULL, p)) {
				fds[0].fd = -1;
				close_fd(&p->pipe_stdout[0]);
			}
		}
		if (fds[1].revents) {
			if (!buf_process(&p->buf_stderr, p->pipe_stderr[0], p->out, APK_OUT_FLUSH, p)) {
				fds[1].fd = -1;
				close_fd(&p->pipe_stderr[0]);
			}
		}
		if (fds[2].revents == POLLOUT) {
			if (!p->is_blob.len) {
				switch (apk_istream_get_all(p->is, &p->is_blob)) {
				case 0:
					break;
				case -APKE_EOF:
					p->is_eof = 1;
					goto stdin_close;
				default:
					goto stdin_close;
				}
			}
			int n = write(p->pipe_stdin[1], p->is_blob.ptr, p->is_blob.len);
			if (n < 0) {
				if (errno == EWOULDBLOCK) break;
				goto stdin_close;
			}
			p->is_blob.ptr += n;
			p->is_blob.len -= n;
		}
		if (fds[2].revents & POLLERR) {
		stdin_close:
			close_fd(&p->pipe_stdin[1]);
			fds[2].fd = -1;
		}
		if (fds[0].revents && break_on_stdout) return 1;
	}
	return apk_process_cleanup(p);
}

int apk_process_run(struct apk_process *p)
{
	return apk_process_handle(p, false);
}

int apk_process_cleanup(struct apk_process *p)
{
	if (p->pid != 0) {
		char buf[APK_EXIT_STATUS_MAX_SIZE];
		if (p->is) apk_istream_close(p->is);
		close_fd(&p->pipe_stdin[1]);
		close_fd(&p->pipe_stdout[0]);
		close_fd(&p->pipe_stderr[0]);

		while (waitpid(p->pid, &p->status, 0) < 0 && errno == EINTR);
		p->pid = 0;

		if (apk_exit_status_str(p->status, buf, sizeof buf))
			apk_err(p->out, "%s: %s", p->argv0, buf);
	}
	if (!WIFEXITED(p->status) || WEXITSTATUS(p->status) != 0) return -1;
	if (p->is && !p->is_eof) return -2;
	return 0;
}

static int process_translate_status(int status)
{
	if (!WIFEXITED(status)) return -EFAULT;
	// Assume wget like return code
	switch (WEXITSTATUS(status)) {
	case 0: return 0;
	case 3: return -EIO;
	case 4: return -ENETUNREACH;
	case 5: return -EACCES;
	case 6: return -EACCES;
	case 7: return -EPROTO;
	default: return -APKE_REMOTE_IO;
	}
}

struct apk_process_istream {
	struct apk_istream is;
	struct apk_process proc;
};

static void process_get_meta(struct apk_istream *is, struct apk_file_meta *meta)
{
}

static ssize_t process_read(struct apk_istream *is, void *ptr, size_t size)
{
	struct apk_process_istream *pis = container_of(is, struct apk_process_istream, is);
	ssize_t r;

	r = apk_process_handle(&pis->proc, true);
	if (r <= 0) return process_translate_status(pis->proc.status);

	r = read(pis->proc.pipe_stdout[0], ptr, size);
	if (r < 0) return -errno;
	return r;
}

static int process_close(struct apk_istream *is)
{
	int r = is->err;
	struct apk_process_istream *pis = container_of(is, struct apk_process_istream, is);

	if (apk_process_cleanup(&pis->proc) < 0 && r >= 0)
		r = process_translate_status(pis->proc.status);
	free(pis);

	return r < 0 ? r : 0;
}

static const struct apk_istream_ops process_istream_ops = {
	.get_meta = process_get_meta,
	.read = process_read,
	.close = process_close,
};

struct apk_istream *apk_process_istream(char * const* argv, struct apk_out *out, const char *logpfx)
{
	struct apk_process_istream *pis;
	int r;

	pis = malloc(sizeof(*pis) + apk_io_bufsize);
	if (pis == NULL) return ERR_PTR(-ENOMEM);

	*pis = (struct apk_process_istream) {
		.is.ops = &process_istream_ops,
		.is.buf = (uint8_t *)(pis + 1),
		.is.buf_size = apk_io_bufsize,
		.is.ptr = (uint8_t *)(pis + 1),
		.is.end = (uint8_t *)(pis + 1),
	};
	r = apk_process_init(&pis->proc, apk_last_path_segment(argv[0]), logpfx, out, NULL);
	if (r != 0) goto err;

	r = apk_process_spawn(&pis->proc, argv[0], argv, NULL);
	if (r != 0) goto err;

	return &pis->is;
err:
	free(pis);
	return ERR_PTR(r);
}
