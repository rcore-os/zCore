/*	$NetBSD: common.c,v 1.31 2016/10/20 21:25:57 joerg Exp $	*/
/*-
 * Copyright (c) 1998-2004 Dag-Erling Coïdan Smørgrav
 * Copyright (c) 2008, 2010 Joerg Sonnenberger <joerg@NetBSD.org>
 * Copyright (c) 2020 Noel Kuntze <noel.kuntze@thermi.consulting>
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
 * $FreeBSD: common.c,v 1.53 2007/12/19 00:26:36 des Exp $
 */

#include <poll.h>
#include <fcntl.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/uio.h>
#include <netinet/in.h>
#include <arpa/inet.h>

#include <ctype.h>
#include <errno.h>
#include <inttypes.h>
#include <netdb.h>
#include <pwd.h>
#include <stdarg.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "fetch.h"
#include "common.h"

/*** Local data **************************************************************/

#ifndef FETCH_NO_TLS
static int ssl_verify_mode = SSL_VERIFY_PEER;
#endif

/*** Error-reporting functions ***********************************************/

void
fetch_check_certificate(int check_cert)
{
#ifdef FETCH_NO_TLS
	(void)check_cert;
#else
	ssl_verify_mode = check_cert ? SSL_VERIFY_PEER : SSL_VERIFY_NONE;
#endif
}

/*
 * Emit status message
 */
void
fetch_info(const char *fmt, ...)
{
	va_list ap;

	va_start(ap, fmt);
	vfprintf(stderr, fmt, ap);
	va_end(ap);
	fputc('\n', stderr);
}


/*** Network-related utility functions ***************************************/

uintmax_t
fetch_parseuint(const char *str, const char **endptr, int radix, uintmax_t max)
{
	uintmax_t val = 0, maxx = max / radix, d;
	const char *p;

	for (p = str; isxdigit((unsigned char)*p); p++) {
		unsigned char ch = (unsigned char)*p;
		if (isdigit(ch))
			d = ch - '0';
		else	d = tolower(ch) - 'a' + 10;
		if (d >= radix || val > maxx) goto err;
		val *= radix;
		if (val > max-d) goto err;
		val += d;
	}
	if (p == str || val > max) goto err;
	*endptr = p;
	return val;
err:
	*endptr = "\xff";
	return 0;
}

/*
 * Return the default port for a scheme
 */
int
fetch_default_port(const char *scheme)
{
	struct servent *se;

	if ((se = getservbyname(scheme, "tcp")) != NULL)
		return (ntohs(se->s_port));
	if (strcasecmp(scheme, SCHEME_HTTP) == 0)
		return (HTTP_DEFAULT_PORT);
	if (strcasecmp(scheme, SCHEME_HTTPS) == 0)
		return (HTTPS_DEFAULT_PORT);
	return (0);
}

/*
 * Return the default proxy port for a scheme
 */
int
fetch_default_proxy_port(const char *scheme)
{
	return (HTTP_DEFAULT_PROXY_PORT);
}


/*
 * Create a connection for an existing descriptor.
 */
conn_t *
fetch_reopen(int sd)
{
	conn_t *conn;

	/* allocate and fill connection structure */
	if ((conn = calloc(1, sizeof(*conn))) == NULL)
		return (NULL);
	conn->ftp_home = NULL;
	conn->cache_url = NULL;
	conn->next_buf = NULL;
	conn->next_len = 0;
	conn->sd = sd;
	conn->buf_events = POLLIN;
	return (conn);
}


/*
 * Bind a socket to a specific local address
 */
int
fetch_bind(int sd, int af, const char *addr)
{
	struct addrinfo hints, *res, *res0;

	memset(&hints, 0, sizeof(hints));
	hints.ai_family = af;
	hints.ai_socktype = SOCK_STREAM;
	hints.ai_protocol = 0;
	if (getaddrinfo(addr, NULL, &hints, &res0))
		return (-1);
	for (res = res0; res; res = res->ai_next) {
		if (bind(sd, res->ai_addr, res->ai_addrlen) == 0)
			return (0);
	}
	return (-1);
}


static int
compute_timeout(const struct timeval *tv)
{
	struct timeval cur;
	int timeout;

	gettimeofday(&cur, NULL);
	timeout = (tv->tv_sec - cur.tv_sec) * 1000 + (tv->tv_usec - cur.tv_usec) / 1000;
	return timeout;
}


/*
 * Establish a TCP connection to the specified port on the specified host.
 */
conn_t *
fetch_connect(struct url *cache_url, struct url *url, int af, int verbose)
{
	conn_t *conn;
	char pbuf[10];
	const char *bindaddr;
	struct addrinfo hints, *res, *res0;
	int sd, error, sock_flags = SOCK_CLOEXEC;

	if (verbose)
		fetch_info("looking up %s", url->host);

	/* look up host name and set up socket address structure */
	snprintf(pbuf, sizeof(pbuf), "%d", url->port);
	memset(&hints, 0, sizeof(hints));
	hints.ai_family = af;
	hints.ai_socktype = SOCK_STREAM;
	hints.ai_protocol = 0;
	if ((error = getaddrinfo(url->host, pbuf, &hints, &res0)) != 0) {
		netdb_seterr(error);
		return (NULL);
	}
	bindaddr = getenv("FETCH_BIND_ADDRESS");

	if (verbose)
		fetch_info("connecting to %s:%d", url->host, url->port);

	if (fetchTimeout)
		sock_flags |= SOCK_NONBLOCK;

	/* try to connect */
	for (sd = -1, res = res0; res; sd = -1, res = res->ai_next) {
		if ((sd = socket(res->ai_family, res->ai_socktype | sock_flags,
			 res->ai_protocol)) == -1)
			continue;
		if (bindaddr != NULL && *bindaddr != '\0' &&
		    fetch_bind(sd, res->ai_family, bindaddr) != 0) {
			fetch_info("failed to bind to '%s'", bindaddr);
			close(sd);
			continue;
		}

		if (connect(sd, res->ai_addr, res->ai_addrlen) == 0)
			break;

		if (fetchTimeout) {
			struct timeval timeout_end;
			struct pollfd pfd = { .fd = sd, .events = POLLOUT };
			int r = -1;

			gettimeofday(&timeout_end, NULL);
			timeout_end.tv_sec += fetchTimeout;

			do {
				int timeout_cur = compute_timeout(&timeout_end);
				if (timeout_cur < 0) {
					errno = ETIMEDOUT;
					break;
				}
				errno = 0;
				r = poll(&pfd, 1, timeout_cur);
				if (r == -1) {
					if (errno == EINTR && fetchRestartCalls)
						continue;
					break;
				}
			} while (pfd.revents == 0);

			if (r == 1 && (pfd.revents & POLLOUT) == POLLOUT) {
				socklen_t len = sizeof(error);
				if (getsockopt(sd, SOL_SOCKET, SO_ERROR, &error, &len) == 0 &&
				    error == 0)
					break;
				errno = error;
			}
		}
		close(sd);
	}
	freeaddrinfo(res0);
	if (sd == -1) {
		fetch_syserr();
		return (NULL);
	}

	if (sock_flags & SOCK_NONBLOCK)
		fcntl(sd, F_SETFL, fcntl(sd, F_GETFL) & ~O_NONBLOCK);

	if ((conn = fetch_reopen(sd)) == NULL) {
		fetch_syserr();
		close(sd);
		return (NULL);
	}
	conn->cache_url = fetchCopyURL(cache_url);
	conn->cache_af = af;
	return (conn);
}

static conn_t *connection_cache;
static int cache_global_limit = 0;
static int cache_per_host_limit = 0;

/*
 * Initialise cache with the given limits.
 */
void
fetchConnectionCacheInit(int global_limit, int per_host_limit)
{

	if (global_limit < 0)
		cache_global_limit = INT_MAX;
	else if (per_host_limit > global_limit)
		cache_global_limit = per_host_limit;
	else
		cache_global_limit = global_limit;
	if (per_host_limit < 0)
		cache_per_host_limit = INT_MAX;
	else
		cache_per_host_limit = per_host_limit;
}

/*
 * Flush cache and free all associated resources.
 */
void
fetchConnectionCacheClose(void)
{
	conn_t *conn;

	while ((conn = connection_cache) != NULL) {
		connection_cache = conn->next_cached;
		(*conn->cache_close)(conn);
	}
}

/*
 * Check connection cache for an existing entry matching
 * protocol/host/port/user/password/family.
 */
conn_t *
fetch_cache_get(const struct url *url, int af)
{
	conn_t *conn, *last_conn = NULL;

	for (conn = connection_cache; conn; conn = conn->next_cached) {
		if (conn->cache_url->port == url->port &&
		    strcmp(conn->cache_url->scheme, url->scheme) == 0 &&
		    strcmp(conn->cache_url->host, url->host) == 0 &&
		    strcmp(conn->cache_url->user, url->user) == 0 &&
		    strcmp(conn->cache_url->pwd, url->pwd) == 0 &&
		    (conn->cache_af == AF_UNSPEC || af == AF_UNSPEC ||
		     conn->cache_af == af)) {
			if (last_conn != NULL)
				last_conn->next_cached = conn->next_cached;
			else
				connection_cache = conn->next_cached;
			return conn;
		}
	}

	return NULL;
}

/*
 * Put the connection back into the cache for reuse.
 * If the connection is freed due to LRU or if the cache
 * is explicitly closed, the given callback is called.
 */
void
fetch_cache_put(conn_t *conn, int (*closecb)(conn_t *))
{
	conn_t *iter, *last, *next_cached;
	int global_count, host_count;

	if (conn->cache_url == NULL || cache_global_limit == 0) {
		(*closecb)(conn);
		return;
	}

	global_count = host_count = 0;
	last = NULL;
	for (iter = connection_cache; iter; last = iter, iter = next_cached) {
		next_cached = iter->next_cached;
		++global_count;
		if (strcmp(conn->cache_url->host, iter->cache_url->host) == 0)
			++host_count;
		if (global_count < cache_global_limit &&
		    host_count < cache_per_host_limit)
			continue;
		--global_count;
		if (last != NULL)
			last->next_cached = iter->next_cached;
		else
			connection_cache = iter->next_cached;
		(*iter->cache_close)(iter);
	}

	conn->cache_close = closecb;
	conn->next_cached = connection_cache;
	connection_cache = conn;
}

/*
 * Configure peer verification based on environment:
 *   1. If compile time #define CA_CERT_FILE is set, and it exists, use it.
 *   2. Use system default CA store settings.
 */
#ifndef FETCH_NO_TLS
static int fetch_ssl_setup_peer_verification(SSL_CTX *ctx, int verbose)
{
	const char *ca_file = NULL;

#ifdef CA_CERT_FILE
	if (access(CA_CERT_FILE, R_OK) == 0) {
		ca_file = CA_CERT_FILE;
#ifdef CA_CRL_FILE
		if (access(CA_CRL_FILE, R_OK) == 0) {
			X509_STORE *crl_store = SSL_CTX_get_cert_store(ctx);
			X509_LOOKUP *crl_lookup = X509_STORE_add_lookup(crl_store, X509_LOOKUP_file());
			if (!crl_lookup || !X509_load_crl_file(crl_lookup, CA_CRL_FILE, X509_FILETYPE_PEM)) {
				fprintf(stderr, "Could not load CRL file %s\n", CA_CRL_FILE);
				return 0;
			}
			X509_STORE_set_flags(crl_store, X509_V_FLAG_CRL_CHECK | X509_V_FLAG_CRL_CHECK_ALL);
		}
#endif
	}
#endif
	if (ca_file)
		SSL_CTX_load_verify_locations(ctx, ca_file, NULL);
	else
		SSL_CTX_set_default_verify_paths(ctx);

	SSL_CTX_set_verify(ctx, ssl_verify_mode, 0);
	return 1;
}

/*
 * Configure client certificate based on environment:
 *  1. Use SSL_CLIENT_{CERT,KEY}_FILE environment variables if set
 *  2. Use compile time set CLIENT_{CERT,KEY}_FILE #define's if set
 *  3. No client certificate used
 *
 * If the key file is not specified, it is assumed that the certificate
 * file is a .pem file containing both the cert and the key.
 */
static int fetch_ssl_setup_client_certificate(SSL_CTX *ctx, int verbose)
{
	const char *cert_file = NULL, *key_file = NULL;

	cert_file = getenv("SSL_CLIENT_CERT_FILE");
	if (cert_file) key_file = getenv("SSL_CLIENT_KEY_FILE");

#ifdef CLIENT_CERT_FILE
	if (!cert_file && access(CLIENT_CERT_FILE, R_OK) == 0) {
		cert_file = CLIENT_CERT_FILE;
#ifdef CLIENT_KEY_FILE
		if (access(CLIENT_KEY_FILE, R_OK) == 0)
			key_file = CLIENT_KEY_FILE;
#endif
	}
#endif
	if (!cert_file) return 1;
	if (!key_file) key_file = cert_file;

	if (verbose) {
		fetch_info("Using client cert file: %s", cert_file);
		fetch_info("Using client key file: %s", key_file);
	}

	if (SSL_CTX_use_certificate_chain_file(ctx, cert_file) != 1) {
		fprintf(stderr, "Could not load client certificate %s\n",
			cert_file);
		return 0;
	}

	if (SSL_CTX_use_PrivateKey_file(ctx, key_file, SSL_FILETYPE_PEM) != 1) {
		fprintf(stderr, "Could not load client key %s\n", key_file);
		return 0;
	}

	return 1;
}

static int map_tls_error(void)
{
	unsigned long err = ERR_peek_error();
	if (ERR_GET_LIB(err) != ERR_LIB_SSL) err = ERR_peek_last_error();
	if (ERR_GET_LIB(err) != ERR_LIB_SSL) return FETCH_ERR_TLS;
	switch (ERR_GET_REASON(err)) {
	case SSL_R_CERTIFICATE_VERIFY_FAILED:
		return FETCH_ERR_TLS_SERVER_CERT_UNTRUSTED;
	case SSL_AD_REASON_OFFSET + TLS1_AD_UNKNOWN_CA:
		return FETCH_ERR_TLS_CLIENT_CERT_UNTRUSTED;
	case SSL_AD_REASON_OFFSET + SSL3_AD_HANDSHAKE_FAILURE:
		return FETCH_ERR_TLS_HANDSHAKE;
	default:
		return FETCH_ERR_TLS;
	}
}
#endif /* !FETCH_NO_TLS */

#ifndef FETCH_NO_TLS
/*
 * Enable SSL on a connection.
 */
int
fetch_ssl(conn_t *conn, const struct url *URL, int verbose)
{
#if OPENSSL_VERSION_NUMBER < 0x10100000L
	conn->ssl_meth = SSLv23_client_method();
#else
	conn->ssl_meth = TLS_client_method();
#endif
	conn->ssl_ctx = SSL_CTX_new(conn->ssl_meth);
	SSL_CTX_set_mode(conn->ssl_ctx, SSL_MODE_AUTO_RETRY);

	if (!fetch_ssl_setup_peer_verification(conn->ssl_ctx, verbose)) goto err;
	if (!fetch_ssl_setup_client_certificate(conn->ssl_ctx, verbose)) goto err;

	conn->ssl = SSL_new(conn->ssl_ctx);
	if (conn->ssl == NULL) goto err;

	conn->buf_events = 0;
	SSL_set_fd(conn->ssl, conn->sd);
	if (!SSL_set_tlsext_host_name(conn->ssl, (char *)(uintptr_t)URL->host)) {
		fprintf(stderr,
		    "TLS server name indication extension failed for host %s\n",
		    URL->host);
		goto err;
	}

	if (SSL_connect(conn->ssl) == -1) {
		tls_seterr(map_tls_error());
		return -1;
	}

	conn->ssl_cert = SSL_get_peer_certificate(conn->ssl);
	if (!conn->ssl_cert) goto err;

	if (getenv("SSL_NO_VERIFY_HOSTNAME") == NULL) {
		if (verbose)
			fetch_info("Verify hostname");
		if (X509_check_host(conn->ssl_cert, URL->host, strlen(URL->host),
				X509_CHECK_FLAG_NO_PARTIAL_WILDCARDS,
				NULL) != 1) {
			if (ssl_verify_mode != SSL_VERIFY_NONE) {
				tls_seterr(FETCH_ERR_TLS_SERVER_CERT_HOSTNAME);
				return -1;
			}
		}
	}

	if (verbose) {
		X509_NAME *name;
		char *str;

		fetch_info("SSL connection established using %s\n", SSL_get_cipher(conn->ssl));
		name = X509_get_subject_name(conn->ssl_cert);
		str = X509_NAME_oneline(name, 0, 0);
		fetch_info("Certificate subject: %s", str);
		free(str);
		name = X509_get_issuer_name(conn->ssl_cert);
		str = X509_NAME_oneline(name, 0, 0);
		fetch_info("Certificate issuer: %s", str);
		free(str);
	}

	return (0);
err:
	tls_seterr(FETCH_ERR_TLS);
	return (-1);
}
#endif /* !FETCH_NO_TLS */

/*
 * Read a character from a connection w/ timeout
 */
ssize_t
fetch_read(conn_t *conn, char *buf, size_t len)
{
	struct timeval timeout_end;
	struct pollfd pfd;
	int timeout_cur;
	ssize_t rlen;
	int r;

	if (len == 0)
		return 0;

	if (conn->next_len != 0) {
		if (conn->next_len < len)
			len = conn->next_len;
		memmove(buf, conn->next_buf, len);
		conn->next_len -= len;
		conn->next_buf += len;
		return len;
	}

	if (fetchTimeout) {
		gettimeofday(&timeout_end, NULL);
		timeout_end.tv_sec += fetchTimeout;
	}

	pfd.fd = conn->sd;
	for (;;) {
		pfd.events = conn->buf_events;
		if (fetchTimeout && pfd.events) {
			do {
				timeout_cur = compute_timeout(&timeout_end);
				if (timeout_cur < 0) {
					errno = ETIMEDOUT;
					fetch_syserr();
					return (-1);
				}
				errno = 0;
				r = poll(&pfd, 1, timeout_cur);
				if (r == -1) {
					if (errno == EINTR && fetchRestartCalls)
						continue;
					fetch_syserr();
					return (-1);
				}
			} while (pfd.revents == 0);
		}

#ifndef FETCH_NO_TLS
		if (conn->ssl != NULL) {
			rlen = SSL_read(conn->ssl, buf, len);
			if (rlen == -1) {
				switch (SSL_get_error(conn->ssl, rlen)) {
				case SSL_ERROR_WANT_READ:
					conn->buf_events = POLLIN;
					break;
				case SSL_ERROR_WANT_WRITE:
					conn->buf_events = POLLOUT;
					break;
				default:
					errno = EIO;
					fetch_syserr();
					return -1;
				}
			} else {
				/* Assume buffering on the SSL layer. */
				conn->buf_events = 0;
			}
		} else {
			rlen = read(conn->sd, buf, len);
		}
#else
		rlen = read(conn->sd, buf, len);
#endif
		if (rlen >= 0)
			break;
	
		if (errno != EINTR || !fetchRestartCalls)
			return (-1);
	}
	return (rlen);
}


/*
 * Read a line of text from a connection w/ timeout
 */
#define MIN_BUF_SIZE 1024

int
fetch_getln(conn_t *conn)
{
	char *tmp, *next;
	size_t tmpsize;
	ssize_t len;

	if (conn->buf == NULL) {
		if ((conn->buf = malloc(MIN_BUF_SIZE)) == NULL) {
			errno = ENOMEM;
			return (-1);
		}
		conn->bufsize = MIN_BUF_SIZE;
	}

	conn->buflen = 0;
	next = NULL;

	do {
		/*
		 * conn->bufsize != conn->buflen at this point,
		 * so the buffer can be NUL-terminated below for
		 * the case of len == 0.
		 */
		len = fetch_read(conn, conn->buf + conn->buflen,
		    conn->bufsize - conn->buflen);
		if (len == -1)
			return (-1);
		if (len == 0)
			break;
		next = memchr(conn->buf + conn->buflen, '\n', len);
		conn->buflen += len;
		if (conn->buflen == conn->bufsize && next == NULL) {
			tmp = conn->buf;
			tmpsize = conn->bufsize * 2;
			if (tmpsize < conn->bufsize) {
				errno = ENOMEM;
				return (-1);
			}
			if ((tmp = realloc(tmp, tmpsize)) == NULL) {
				errno = ENOMEM;
				return (-1);
			}
			conn->buf = tmp;
			conn->bufsize = tmpsize;
		}
	} while (next == NULL);

	if (next != NULL) {
		*next = '\0';
		conn->next_buf = next + 1;
		conn->next_len = conn->buflen - (conn->next_buf - conn->buf);
		conn->buflen = next - conn->buf;
	} else {
		conn->buf[conn->buflen] = '\0';
		conn->next_len = 0;
	}
	return (0);
}

/*
 * Write a vector to a connection w/ timeout
 * Note: can modify the iovec.
 */
ssize_t
fetch_write(conn_t *conn, const void *buf, size_t len)
{
	struct timeval now, timeout, waittv;
	fd_set writefds;
	ssize_t wlen, total;
	int r;

	if (fetchTimeout) {
		FD_ZERO(&writefds);
		gettimeofday(&timeout, NULL);
		timeout.tv_sec += fetchTimeout;
	}

	total = 0;
	while (len) {
		while (fetchTimeout && !FD_ISSET(conn->sd, &writefds)) {
			FD_SET(conn->sd, &writefds);
			gettimeofday(&now, NULL);
			waittv.tv_sec = timeout.tv_sec - now.tv_sec;
			waittv.tv_usec = timeout.tv_usec - now.tv_usec;
			if (waittv.tv_usec < 0) {
				waittv.tv_usec += 1000000;
				waittv.tv_sec--;
			}
			if (waittv.tv_sec < 0) {
				errno = ETIMEDOUT;
				fetch_syserr();
				return (-1);
			}
			errno = 0;
			r = select(conn->sd + 1, NULL, &writefds, NULL, &waittv);
			if (r == -1) {
				if (errno == EINTR && fetchRestartCalls)
					continue;
				return (-1);
			}
		}
		errno = 0;
#ifndef FETCH_NO_TLS
		if (conn->ssl != NULL)
			wlen = SSL_write(conn->ssl, buf, len);
		else
			wlen = send(conn->sd, buf, len, MSG_NOSIGNAL);
#else
		wlen = send(conn->sd, buf, len, MSG_NOSIGNAL);
#endif
		if (wlen == 0) {
			/* we consider a short write a failure */
			errno = EPIPE;
			fetch_syserr();
			return (-1);
		}
		if (wlen < 0) {
			if (errno == EINTR && fetchRestartCalls)
				continue;
			return (-1);
		}
		total += wlen;
		buf = (const char *)buf + wlen;
		len -= wlen;
	}
	return (total);
}


/*
 * Close connection
 */
int
fetch_close(conn_t *conn)
{
	int ret;

#ifndef FETCH_NO_TLS
	if (conn->ssl) {
		SSL_shutdown(conn->ssl);
		SSL_set_connect_state(conn->ssl);
		SSL_free(conn->ssl);
	}
	if (conn->ssl_ctx) {
		SSL_CTX_free(conn->ssl_ctx);
	}
	if (conn->ssl_cert) {
		X509_free(conn->ssl_cert);
	}
#endif

	ret = close(conn->sd);
	if (conn->cache_url)
		fetchFreeURL(conn->cache_url);
	free(conn->ftp_home);
	free(conn->buf);
	free(conn);
	return (ret);
}


/*** Directory-related utility functions *************************************/

int
fetch_add_entry(struct url_list *ue, struct url *base, const char *name,
    int pre_quoted)
{
	struct url *tmp;
	char *tmp_name;
	size_t base_doc_len, name_len, i;
	unsigned char c;

	if (strchr(name, '/') != NULL ||
	    strcmp(name, "..") == 0 ||
	    strcmp(name, ".") == 0)
		return 0;

	if (strcmp(base->doc, "/") == 0)
		base_doc_len = 0;
	else
		base_doc_len = strlen(base->doc);

	name_len = 1;
	for (i = 0; name[i] != '\0'; ++i) {
		if ((!pre_quoted && name[i] == '%') ||
		    !fetch_urlpath_safe(name[i]))
			name_len += 3;
		else
			++name_len;
	}

	tmp_name = malloc( base_doc_len + name_len + 1);
	if (tmp_name == NULL) {
		errno = ENOMEM;
		fetch_syserr();
		return (-1);
	}

	if (ue->length + 1 >= ue->alloc_size) {
		tmp = realloc(ue->urls, (ue->alloc_size * 2 + 1) * sizeof(*tmp));
		if (tmp == NULL) {
			free(tmp_name);
			errno = ENOMEM;
			fetch_syserr();
			return (-1);
		}
		ue->alloc_size = ue->alloc_size * 2 + 1;
		ue->urls = tmp;
	}

	tmp = ue->urls + ue->length;
	strcpy(tmp->scheme, base->scheme);
	strcpy(tmp->user, base->user);
	strcpy(tmp->pwd, base->pwd);
	strcpy(tmp->host, base->host);
	tmp->port = base->port;
	tmp->doc = tmp_name;
	memcpy(tmp->doc, base->doc, base_doc_len);
	tmp->doc[base_doc_len] = '/';

	for (i = base_doc_len + 1; *name != '\0'; ++name) {
		if ((!pre_quoted && *name == '%') ||
		    !fetch_urlpath_safe(*name)) {
			tmp->doc[i++] = '%';
			c = (unsigned char)*name / 16;
			if (c < 10)
				tmp->doc[i++] = '0' + c;
			else
				tmp->doc[i++] = 'a' - 10 + c;
			c = (unsigned char)*name % 16;
			if (c < 10)
				tmp->doc[i++] = '0' + c;
			else
				tmp->doc[i++] = 'a' - 10 + c;
		} else {
			tmp->doc[i++] = *name;
		}
	}
	tmp->doc[i] = '\0';

	tmp->offset = 0;
	tmp->length = 0;
	tmp->last_modified = -1;

	++ue->length;

	return (0);
}

void
fetchInitURLList(struct url_list *ue)
{
	ue->length = ue->alloc_size = 0;
	ue->urls = NULL;
}

int
fetchAppendURLList(struct url_list *dst, const struct url_list *src)
{
	size_t i, j, len;

	len = dst->length + src->length;
	if (len > dst->alloc_size) {
		struct url *tmp;

		tmp = realloc(dst->urls, len * sizeof(*tmp));
		if (tmp == NULL) {
			errno = ENOMEM;
			fetch_syserr();
			return (-1);
		}
		dst->alloc_size = len;
		dst->urls = tmp;
	}

	for (i = 0, j = dst->length; i < src->length; ++i, ++j) {
		dst->urls[j] = src->urls[i];
		dst->urls[j].doc = strdup(src->urls[i].doc);
		if (dst->urls[j].doc == NULL) {
			while (i-- > 0)
				free(dst->urls[j].doc);
			fetch_syserr();
			return -1;
		}
	}
	dst->length = len;

	return 0;
}

void
fetchFreeURLList(struct url_list *ue)
{
	size_t i;

	for (i = 0; i < ue->length; ++i)
		free(ue->urls[i].doc);
	free(ue->urls);
	ue->length = ue->alloc_size = 0;
}


/*** Authentication-related utility functions ********************************/

static const char *
fetch_read_word(FILE *f)
{
	static char word[4096];

	if (fscanf(f, " %4095s ", word) != 1)
		return (NULL);
	return (word);
}

/*
 * Get authentication data for a URL from .netrc
 */
int
fetch_netrc_auth(struct url *url)
{
	char fn[PATH_MAX];
	const char *word;
	char *p;
	FILE *f;

	if ((p = getenv("NETRC")) != NULL) {
		if (snprintf(fn, sizeof(fn), "%s", p) >= (int)sizeof(fn)) {
			fetch_info("$NETRC specifies a file name "
			    "longer than PATH_MAX");
			return (-1);
		}
	} else {
		if ((p = getenv("HOME")) != NULL) {
			struct passwd *pwd;

			if ((pwd = getpwuid(getuid())) == NULL ||
			    (p = pwd->pw_dir) == NULL)
				return (-1);
		}
		if (snprintf(fn, sizeof(fn), "%s/.netrc", p) >= (int)sizeof(fn))
			return (-1);
	}

	if ((f = fopen(fn, "r")) == NULL)
		return (-1);
	while ((word = fetch_read_word(f)) != NULL) {
		if (strcmp(word, "default") == 0)
			break;
		if (strcmp(word, "machine") == 0 &&
		    (word = fetch_read_word(f)) != NULL &&
		    strcasecmp(word, url->host) == 0) {
			break;
		}
	}
	if (word == NULL)
		goto ferr;
	while ((word = fetch_read_word(f)) != NULL) {
		if (strcmp(word, "login") == 0) {
			if ((word = fetch_read_word(f)) == NULL)
				goto ferr;
			if (snprintf(url->user, sizeof(url->user),
				"%s", word) > (int)sizeof(url->user)) {
				url->user[0] = '\0';
				fetch_info("login name in .netrc is too long (exceeds %d bytes)",
				    (int)sizeof(url->user) - 1);
				goto ferr;
			}
		} else if (strcmp(word, "password") == 0) {
			if ((word = fetch_read_word(f)) == NULL)
				goto ferr;
			if (snprintf(url->pwd, sizeof(url->pwd),
				"%s", word) > (int)sizeof(url->pwd)) {
				url->pwd[0] = '\0';
				fetch_info("password in .netrc is too long (exceeds %d bytes)",
				    (int)sizeof(url->pwd) - 1);
				goto ferr;
			}
		} else if (strcmp(word, "account") == 0) {
			if ((word = fetch_read_word(f)) == NULL)
				goto ferr;
			/* XXX not supported! */
		} else {
			break;
		}
	}
	fclose(f);
	return (0);
 ferr:
	fclose(f);
	return (-1);
}

#define MAX_ADDRESS_BYTES	sizeof(struct in6_addr)
#define MAX_ADDRESS_STRING	(4*8+1)
#define MAX_CIDR_STRING		(MAX_ADDRESS_STRING+4)

static size_t host_to_address(uint8_t *buf, size_t buf_len, const char *host, size_t len)
{
	char tmp[MAX_ADDRESS_STRING];

	if (len >= sizeof tmp) return 0;
	if (buf_len < sizeof(struct in6_addr)) return 0;

	/* Make zero terminated copy of the hostname */
	memcpy(tmp, host, len);
	tmp[len] = 0;

	if (inet_pton(AF_INET, tmp, (struct in_addr *) buf))
		return sizeof(struct in_addr);
	if (inet_pton(AF_INET6, tmp, (struct in6_addr *) buf))
		return sizeof(struct in6_addr);
	return 0;
}

static int bitcmp(const uint8_t *a, const uint8_t *b, int len)
{
	int bytes, bits, mask, r;

	bytes = len / 8;
	bits  = len % 8;
	if (bytes != 0) {
		r = memcmp(a, b, bytes);
		if (r != 0) return r;
	}
	if (bits != 0) {
		mask = (0xff << (8 - bits)) & 0xff;
		return ((int) (a[bytes] & mask)) - ((int) (b[bytes] & mask));
	}
	return 0;
}

static int cidr_match(const uint8_t *addr, size_t addr_len, const char *cidr, size_t cidr_len)
{
	const char *slash;
	uint8_t cidr_addr[MAX_ADDRESS_BYTES];
	size_t cidr_addrlen;
	long bits;

	if (!addr_len || cidr_len > MAX_CIDR_STRING) return 0;
	slash = memchr(cidr, '/', cidr_len);
	if (!slash) return 0;
	bits = strtol(slash + 1, NULL, 10);
	if (!bits || bits > 128) return 0;

	cidr_addrlen = host_to_address(cidr_addr, sizeof cidr_addr, cidr, slash - cidr);
	if (cidr_addrlen != addr_len || bits > addr_len*8) return 0;
	return bitcmp(cidr_addr, addr, bits) == 0;
}

/*
 * The no_proxy environment variable specifies a set of domains for
 * which the proxy should not be consulted; the contents is a comma-,
 * or space-separated list of domain names.  A single asterisk will
 * override all proxy variables and no transactions will be proxied
 * (for compatability with lynx and curl, see the discussion at
 * <http://curl.haxx.se/mail/archive_pre_oct_99/0009.html>).
 */
int
fetch_no_proxy_match(const char *host)
{
	const char *no_proxy, *p, *q;
	uint8_t addr[MAX_ADDRESS_BYTES];
	size_t h_len, d_len, addr_len;

	if ((no_proxy = getenv("NO_PROXY")) == NULL &&
	    (no_proxy = getenv("no_proxy")) == NULL)
		return (0);

	/* asterisk matches any hostname */
	if (strcmp(no_proxy, "*") == 0)
		return (1);

	h_len = strlen(host);
	addr_len = host_to_address(addr, sizeof addr, host, h_len);
	p = no_proxy;
	do {
		/* position p at the beginning of a domain suffix */
		while (*p == ',' || isspace((unsigned char)*p))
			p++;

		/* position q at the first separator character */
		for (q = p; *q; ++q)
			if (*q == ',' || isspace((unsigned char)*q))
				break;

		d_len = q - p;
		if (d_len > 0 && h_len >= d_len &&
		    strncasecmp(host + h_len - d_len,
			p, d_len) == 0) {
			/* domain name matches */
			return (1);
		}

		if (cidr_match(addr, addr_len, p, d_len)) {
			return (1);
		}

		p = q + 1;
	} while (*q);

	return (0);
}

struct fetchIO {
	void *io_cookie;
	ssize_t (*io_read)(void *, void *, size_t);
	ssize_t (*io_write)(void *, const void *, size_t);
	void (*io_close)(void *);
};

void
fetchIO_close(fetchIO *f)
{
	if (f->io_close != NULL)
		(*f->io_close)(f->io_cookie);

	free(f);
}

fetchIO *
fetchIO_unopen(void *io_cookie, ssize_t (*io_read)(void *, void *, size_t),
    ssize_t (*io_write)(void *, const void *, size_t),
    void (*io_close)(void *))
{
	fetchIO *f;

	f = malloc(sizeof(*f));
	if (f == NULL)
		return f;

	f->io_cookie = io_cookie;
	f->io_read = io_read;
	f->io_write = io_write;
	f->io_close = io_close;

	return f;
}

ssize_t
fetchIO_read(fetchIO *f, void *buf, size_t len)
{
	if (f->io_read == NULL)
		return EBADF;
	return (*f->io_read)(f->io_cookie, buf, len);
}

ssize_t
fetchIO_write(fetchIO *f, const void *buf, size_t len)
{
	if (f->io_read == NULL)
		return EBADF;
	return (*f->io_write)(f->io_cookie, buf, len);
}
