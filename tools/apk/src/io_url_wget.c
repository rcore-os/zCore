/* io_url_wget.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include "apk_io.h"
#include "apk_process.h"

static char wget_timeout[16];
static bool wget_no_check_certificate;
static struct apk_out *wget_out;

struct apk_istream *apk_io_url_istream(const char *url, time_t since)
{
	char *argv[16];
	int i = 0;

	argv[i++] = "wget";
	argv[i++] = "-q";
	argv[i++] = "-T";
	argv[i++] = wget_timeout;
	if (wget_no_check_certificate) argv[i++] = "--no-check-certificate";
	argv[i++] = (char *) url;
	argv[i++] = "-O";
	argv[i++] = "-";
	argv[i++] = 0;

	return apk_process_istream(argv, wget_out, "wget");
}

void apk_io_url_check_certificate(bool check_cert)
{
	wget_no_check_certificate = !check_cert;
}

void apk_io_url_set_timeout(int timeout)
{
	apk_fmt(wget_timeout, sizeof wget_timeout, "%d", timeout);
}

void apk_io_url_set_redirect_callback(void (*cb)(int, const char *))
{
}

void apk_io_url_init(struct apk_out *out)
{
	wget_out = out;
}
