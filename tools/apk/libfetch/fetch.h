/*	$NetBSD: fetch.h,v 1.16 2010/01/22 13:21:09 joerg Exp $	*/
/*-
 * Copyright (c) 1998-2004 Dag-Erling Coïdan Smørgrav
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer
 *    in this position and unchanged.
 * 2. Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in the
 *    documentation and/or other materials provided with the distribution.
 * 3. The name of the author may not be used to endorse or promote products
 *    derived from this software without specific prior written permission
 *
 * THIS SOFTWARE IS PROVIDED BY THE AUTHOR ``AS IS'' AND ANY EXPRESS OR
 * IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES
 * OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED.
 * IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY DIRECT, INDIRECT,
 * INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT
 * NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
 * DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
 * THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
 * (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF
 * THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 *
 * $FreeBSD: fetch.h,v 1.26 2004/09/21 18:35:20 des Exp $
 */

#ifndef _FETCH_H_INCLUDED
#define _FETCH_H_INCLUDED

#include <sys/types.h>
#include <limits.h>
#include <stdio.h>

#define _LIBFETCH_VER "libfetch/2.0"

#define URL_HOSTLEN 255
#define URL_SCHEMELEN 16
#define URL_USERLEN 256
#define URL_PWDLEN 4096

typedef struct fetchIO fetchIO;

struct url {
	char		 scheme[URL_SCHEMELEN + 1];
	char		 user[URL_USERLEN + 1];
	char		 pwd[URL_PWDLEN + 1];
	char		 host[URL_HOSTLEN + 1];
	int		 port;
	char		*doc;
	off_t		 offset;
	size_t		 length;
	time_t		 last_modified;
};

struct url_stat {
	off_t		 size;
	time_t		 atime;
	time_t		 mtime;
};

struct url_list {
	size_t		 length;
	size_t		 alloc_size;
	struct url	*urls;
};

/* Recognized schemes */
#define SCHEME_HTTP	"http"
#define SCHEME_HTTPS	"https"

enum {
	/* Error categories */
	FETCH_ERRCAT_FETCH = 0,
	FETCH_ERRCAT_ERRNO,
	FETCH_ERRCAT_NETDB,
	FETCH_ERRCAT_HTTP,
	FETCH_ERRCAT_URL,
	FETCH_ERRCAT_TLS,

	/* Error FETCH category codes */
	FETCH_OK = 0,
	FETCH_ERR_UNKNOWN,
	FETCH_ERR_UNCHANGED,

	/* Error URL category codes */
	FETCH_ERR_URL_MALFORMED = 1,
	FETCH_ERR_URL_BAD_SCHEME,
	FETCH_ERR_URL_BAD_PORT,
	FETCH_ERR_URL_BAD_HOST,
	FETCH_ERR_URL_BAD_AUTH,

	/* Error TLS category codes */
	FETCH_ERR_TLS = 1,
	FETCH_ERR_TLS_SERVER_CERT_ABSENT,
	FETCH_ERR_TLS_SERVER_CERT_HOSTNAME,
	FETCH_ERR_TLS_SERVER_CERT_UNTRUSTED,
	FETCH_ERR_TLS_CLIENT_CERT_UNTRUSTED,
	FETCH_ERR_TLS_HANDSHAKE,
};

struct fetch_error {
	unsigned int category;
	int code;
};

#if defined(__cplusplus)
extern "C" {
#endif

void		fetch_check_certificate(int check_cert);

void		fetchIO_close(fetchIO *);
ssize_t		fetchIO_read(fetchIO *, void *, size_t);
ssize_t		fetchIO_write(fetchIO *, const void *, size_t);

/* HTTP-specific functions */
fetchIO		*fetchXGetHTTP(struct url *, struct url_stat *, const char *);
fetchIO		*fetchGetHTTP(struct url *, const char *);
fetchIO		*fetchPutHTTP(struct url *, const char *);
int		 fetchStatHTTP(struct url *, struct url_stat *, const char *);
int		 fetchListHTTP(struct url_list *, struct url *, const char *,
		    const char *);

/* Generic functions */
fetchIO		*fetchXGetURL(const char *, struct url_stat *, const char *);
fetchIO		*fetchGetURL(const char *, const char *);
fetchIO		*fetchPutURL(const char *, const char *);
int		 fetchStatURL(const char *, struct url_stat *, const char *);
int		 fetchListURL(struct url_list *, const char *, const char *,
		    const char *);
fetchIO		*fetchXGet(struct url *, struct url_stat *, const char *);
fetchIO		*fetchGet(struct url *, const char *);
fetchIO		*fetchPut(struct url *, const char *);
int		 fetchStat(struct url *, struct url_stat *, const char *);
int		 fetchList(struct url_list *, struct url *, const char *,
		    const char *);

/* URL parsing */
struct url	*fetchMakeURL(const char *, const char *, int,
		     const char *, const char *, const char *);
struct url	*fetchParseURL(const char *);
struct url	*fetchCopyURL(const struct url *);
char		*fetchStringifyURL(const struct url *);
void		 fetchFreeURL(struct url *);

/* URL listening */
void		 fetchInitURLList(struct url_list *);
int		 fetchAppendURLList(struct url_list *, const struct url_list *);
void		 fetchFreeURLList(struct url_list *);
char		*fetchUnquotePath(struct url *);
char		*fetchUnquoteFilename(struct url *);

/* Connection caching */
void		 fetchConnectionCacheInit(int, int);
void		 fetchConnectionCacheClose(void);

/* Redirects */
typedef void (*fetch_redirect_t)(int, const struct url *, const struct url *);
extern fetch_redirect_t	 fetchRedirectMethod;

/* Authentication */
typedef int (*auth_t)(struct url *);
extern auth_t		 fetchAuthMethod;

/* Last error code */
extern struct fetch_error fetchLastErrCode;

/* I/O timeout */
extern int		 fetchTimeout;

/* Restart interrupted syscalls */
extern volatile int	 fetchRestartCalls;

/* Extra verbosity */
extern int		 fetchDebug;

#if defined(__cplusplus)
}
#endif

#endif
