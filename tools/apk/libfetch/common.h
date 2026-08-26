/*	$NetBSD: common.h,v 1.24 2016/10/20 21:25:57 joerg Exp $	*/
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
 * $FreeBSD: common.h,v 1.30 2007/12/18 11:03:07 des Exp $
 */

#ifndef _COMMON_H_INCLUDED
#define _COMMON_H_INCLUDED

#define HTTP_DEFAULT_PORT	80
#define HTTPS_DEFAULT_PORT	443
#define HTTP_DEFAULT_PROXY_PORT	3128

#include <sys/types.h>
#include <limits.h>
#include <inttypes.h>
#ifndef FETCH_NO_TLS
#include "openssl-compat.h"
#endif

#if defined(__GNUC__) && __GNUC__ >= 3
#define LIBFETCH_PRINTFLIKE(fmtarg, firstvararg)	\
	    __attribute__((__format__ (__printf__, fmtarg, firstvararg)))
#else
#define LIBFETCH_PRINTFLIKE(fmtarg, firstvararg)
#endif

#if !defined(__sun) && !defined(__hpux) && !defined(__INTERIX) && \
    !defined(__digital__) && !defined(__linux) && !defined(__MINT__) && \
    !defined(__sgi) && !defined(__minix) && !defined(__CYGWIN__)
#define HAVE_SA_LEN
#endif

#ifndef IPPORT_MAX
# define IPPORT_MAX 65535
#endif

#ifndef OFF_MAX
# define OFF_MAX (((((off_t)1 << (sizeof(off_t) * CHAR_BIT - 2)) - 1) << 1) + 1)
#endif

/* Connection */
typedef struct fetchconn conn_t;

struct fetchconn {
	int		 sd;		/* socket descriptor */
	char		*buf;		/* buffer */
	size_t		 bufsize;	/* buffer size */
	size_t		 buflen;	/* length of buffer contents */
	int		 buf_events;	/* poll flags for the next cycle */
	char		*next_buf;	/* pending buffer, e.g. after getln */
	size_t		 next_len;	/* size of pending buffer */
	int		 err;		/* last protocol reply code */
#ifndef FETCH_NO_TLS
	SSL		*ssl;		/* SSL handle */
	SSL_CTX		*ssl_ctx;	/* SSL context */
	X509		*ssl_cert;	/* server certificate */
	const SSL_METHOD *ssl_meth;	/* SSL method */
#endif
	char		*ftp_home;
	struct url	*cache_url;
	int		cache_af;
	int		(*cache_close)(conn_t *);
	conn_t		*next_cached;
};

void		 fetch_info(const char *, ...)  LIBFETCH_PRINTFLIKE(1, 2);
uintmax_t	 fetch_parseuint(const char *p, const char **endptr, int radix, uintmax_t max);
int		 fetch_default_port(const char *);
int		 fetch_default_proxy_port(const char *);
int		 fetch_bind(int, int, const char *);
conn_t		*fetch_cache_get(const struct url *, int);
void		 fetch_cache_put(conn_t *, int (*)(conn_t *));
conn_t		*fetch_connect(struct url *, struct url *, int, int);
conn_t		*fetch_reopen(int);
#ifndef FETCH_NO_TLS
int		 fetch_ssl(conn_t *, const struct url *, int);
#else
static inline int fetch_ssl(conn_t *conn, const struct url *URL, int verbose) {
	(void)conn; (void)URL; (void)verbose;
	return -1;
}
#endif
ssize_t		 fetch_read(conn_t *, char *, size_t);
int		 fetch_getln(conn_t *);
ssize_t		 fetch_write(conn_t *, const void *, size_t);
int		 fetch_close(conn_t *);
int		 fetch_add_entry(struct url_list *, struct url *, const char *, int);
int		 fetch_netrc_auth(struct url *url);
int		 fetch_no_proxy_match(const char *);
int		 fetch_urlpath_safe(char);

static inline void _fetch_seterr(unsigned int category, int code) {
	fetchLastErrCode = (struct fetch_error) { .category = category, .code = code };
}
static inline void fetch_syserr(void) {
	_fetch_seterr(FETCH_ERRCAT_ERRNO, errno);
}

#define fetch_seterr(n)	_fetch_seterr(FETCH_ERRCAT_FETCH, n)
#define url_seterr(n)	_fetch_seterr(FETCH_ERRCAT_URL, FETCH_ERR_##n)
#define http_seterr(n)	_fetch_seterr(FETCH_ERRCAT_HTTP, n)
#define netdb_seterr(n)	_fetch_seterr(FETCH_ERRCAT_NETDB, n)
#define tls_seterr(n)	_fetch_seterr(FETCH_ERRCAT_TLS, n)

fetchIO		*fetchIO_unopen(void *, ssize_t (*)(void *, void *, size_t),
    ssize_t (*)(void *, const void *, size_t), void (*)(void *));

/*
 * Check whether a particular flag is set
 */
#define CHECK_FLAG(x)	(flags && strchr(flags, (x)))

#endif
