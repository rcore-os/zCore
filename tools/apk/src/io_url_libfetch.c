/* io_url_libfetch.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <stdio.h>
#include <fcntl.h>
#include <errno.h>
#include <stdlib.h>
#include <unistd.h>

#include <fetch.h>
#include <netdb.h>

#include "apk_io.h"

struct apk_fetch_istream {
	struct apk_istream is;
	fetchIO *fetchIO;
	struct url_stat urlstat;
};

struct maperr {
	int fetch;
	unsigned int apk;
};

static int fetch_maperr(const struct maperr *map, size_t mapsz, int ec, int default_apkerr)
{
	for (; mapsz; mapsz--, map++) if (map->fetch == ec) return map->apk;
	return default_apkerr;
}

static int fetch_maperror(struct fetch_error fe)
{
	static const struct maperr fetch_err[] = {
		{ FETCH_OK,			0, },
		{ FETCH_ERR_UNKNOWN,		EIO },
		{ FETCH_ERR_UNCHANGED,		APKE_FILE_UNCHANGED },
	};
	static const struct maperr tls_err[] = {
		{ FETCH_ERR_TLS,			APKE_TLS_ERROR },
		{ FETCH_ERR_TLS_SERVER_CERT_HOSTNAME,	APKE_TLS_SERVER_CERT_HOSTNAME },
		{ FETCH_ERR_TLS_SERVER_CERT_UNTRUSTED,	APKE_TLS_SERVER_CERT_UNTRUSTED },
		{ FETCH_ERR_TLS_CLIENT_CERT_UNTRUSTED,	APKE_TLS_CLIENT_CERT_UNTRUSTED },
		{ FETCH_ERR_TLS_HANDSHAKE,		APKE_TLS_HANDSHAKE },
	};
	static const struct maperr netdb_err[] = {
		{ EAI_ADDRFAMILY, 	APKE_DNS_ADDRESS_FAMILY },
		{ EAI_NODATA,		APKE_DNS_NO_DATA },
		{ EAI_AGAIN,		APKE_DNS_AGAIN },
		{ EAI_FAIL,		APKE_DNS_FAIL },
		{ EAI_NONAME,		APKE_DNS_NO_NAME },
	};
	static const struct maperr http_err[] = {
		{ 304, APKE_FILE_UNCHANGED },
		{ 400, APKE_HTTP_400_BAD_REQUEST },
		{ 401, APKE_HTTP_401_UNAUTHORIZED },
		{ 403, APKE_HTTP_403_FORBIDDEN },
		{ 404, APKE_HTTP_404_NOT_FOUND },
		{ 405, APKE_HTTP_405_METHOD_NOT_ALLOWED },
		{ 406, APKE_HTTP_406_NOT_ACCEPTABLE },
		{ 407, APKE_HTTP_407_PROXY_AUTH_REQUIRED },
		{ 408, APKE_HTTP_408_TIMEOUT },
		{ 500, APKE_HTTP_500_INTERNAL_SERVER_ERROR },
		{ 501, APKE_HTTP_501_NOT_IMPLEMENTED },
		{ 502, APKE_HTTP_502_BAD_GATEWAY },
		{ 503, APKE_HTTP_503_SERVICE_UNAVAILABLE, },
		{ 504, APKE_HTTP_504_GATEWAY_TIMEOUT },
	};

	switch (fe.category) {
	case FETCH_ERRCAT_FETCH:
		return fetch_maperr(fetch_err, ARRAY_SIZE(fetch_err), fe.code, EIO);
	case FETCH_ERRCAT_URL:
		return APKE_URL_FORMAT;
	case FETCH_ERRCAT_ERRNO:
		return fe.code ?: EIO;
	case FETCH_ERRCAT_NETDB:
		return fetch_maperr(netdb_err, ARRAY_SIZE(netdb_err), fe.code, APKE_DNS_FAIL);
	case FETCH_ERRCAT_HTTP:
		return fetch_maperr(http_err, ARRAY_SIZE(http_err), fe.code, APKE_HTTP_UNKNOWN);
	case FETCH_ERRCAT_TLS:
		return fetch_maperr(tls_err, ARRAY_SIZE(tls_err), fe.code, APKE_TLS_ERROR);
	default:
		return EIO;
	}
}

static void fetch_get_meta(struct apk_istream *is, struct apk_file_meta *meta)
{
	struct apk_fetch_istream *fis = container_of(is, struct apk_fetch_istream, is);

	*meta = (struct apk_file_meta) {
		.atime = fis->urlstat.atime,
		.mtime = fis->urlstat.mtime,
	};
}

static ssize_t fetch_read(struct apk_istream *is, void *ptr, size_t size)
{
	struct apk_fetch_istream *fis = container_of(is, struct apk_fetch_istream, is);
	ssize_t r;

	r = fetchIO_read(fis->fetchIO, ptr, size);
	if (r < 0) return -EIO;
	return r;
}

static int fetch_close(struct apk_istream *is)
{
	int r = is->err;
	struct apk_fetch_istream *fis = container_of(is, struct apk_fetch_istream, is);

	fetchIO_close(fis->fetchIO);
	free(fis);
	return r < 0 ? r : 0;
}

static const struct apk_istream_ops fetch_istream_ops = {
	.get_meta = fetch_get_meta,
	.read = fetch_read,
	.close = fetch_close,
};

struct apk_istream *apk_io_url_istream(const char *url, time_t since)
{
	struct apk_fetch_istream *fis = NULL;
	struct url *u;
	char *flags = "Ci";
	fetchIO *io = NULL;
	int rc = -EIO;

	u = fetchParseURL(url);
	if (!u) {
		rc = -APKE_URL_FORMAT;
		goto err;
	}
	fis = malloc(sizeof *fis + apk_io_bufsize);
	if (!fis) {
		rc = -ENOMEM;
		goto err;
	}

	if (since != APK_ISTREAM_FORCE_REFRESH) {
		u->last_modified = since;
		flags = "i";
	}

	io = fetchXGet(u, &fis->urlstat, flags);
	if (!io) {
		rc = -fetch_maperror(fetchLastErrCode);
		goto err;
	}

	*fis = (struct apk_fetch_istream) {
		.is.ops = &fetch_istream_ops,
		.is.buf = (uint8_t*)(fis+1),
		.is.buf_size = apk_io_bufsize,
		.is.ptr = (uint8_t*)(fis+1),
		.is.end = (uint8_t*)(fis+1),
		.fetchIO = io,
		.urlstat = fis->urlstat,
	};
	fetchFreeURL(u);

	return &fis->is;
err:
	if (u) fetchFreeURL(u);
	if (io) fetchIO_close(io);
	if (fis) free(fis);
	return ERR_PTR(rc);
}

static void (*io_url_redirect_callback)(int, const char *);

static void fetch_redirect(int code, const struct url *cur, const struct url *next)
{
	char *url;

	switch (code) {
	case 301: // Moved Permanently
	case 308: // Permanent Redirect
		url = fetchStringifyURL(next);
		io_url_redirect_callback(code, url);
		free(url);
		break;
	}
}

void apk_io_url_check_certificate(bool check_cert)
{
	fetch_check_certificate(check_cert);
}

void apk_io_url_set_timeout(int timeout)
{
	fetchTimeout = timeout;
}

void apk_io_url_set_redirect_callback(void (*cb)(int, const char *))
{
	fetchRedirectMethod = cb ? fetch_redirect : NULL;
	io_url_redirect_callback = cb;
}

static void apk_io_url_fini(void)
{
	fetchConnectionCacheClose();
}

void apk_io_url_init(struct apk_out *out)
{
	fetchConnectionCacheInit(32, 4);
	atexit(apk_io_url_fini);
}
