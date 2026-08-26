/* apk_applet.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include <errno.h>
#include <getopt.h>
#include "apk_defines.h"
#include "apk_database.h"

#define __APK_OPT_ENUM(_enum,__desc) _enum,
#define __APK_OPT_DESC(_enum,__desc) __desc "\x00"

#define APK_OPT_SH(x)		"\xa0" x
#define APK_OPT_AUTO		"\xaa"
#define APK_OPT_BOOL		"\xab"
#define APK_OPT_ARG		"\xaf"

#define APK_OPTARG_VAL(val)	((intptr_t)(val))
#define APK_OPTARG(val)		((void*)(intptr_t)(val))

#define APK_OPTIONS(var_name, init_macro) \
	enum { init_macro(__APK_OPT_ENUM) }; \
	static const char var_name[] = init_macro(__APK_OPT_DESC);

#define APK_OPTIONS_EXT(var_name, init_macro) \
	enum { init_macro(__APK_OPT_ENUM) }; \
	const char var_name[] = init_macro(__APK_OPT_DESC);

#define APK_OPTIONS_INIT 0xffff00

static inline void apk_opt_set_flag(const char *optarg, unsigned int flag, unsigned int *flags)
{
	if (optarg == APK_OPTARG(APK_YES)) *flags |= flag;
	else *flags &= ~flag;
}
static inline void apk_opt_set_flag_invert(const char *optarg, unsigned int flag, unsigned int *flags)
{
	if (optarg == APK_OPTARG(APK_NO)) *flags |= flag;
	else *flags &= ~flag;
}

struct apk_applet {
	struct list_head node;

	const char *name;
	const char *options_desc;

	unsigned short optgroup_commit : 1;
	unsigned short optgroup_generation : 1;
	unsigned short optgroup_query : 1;
	unsigned short remove_empty_arguments : 1;
	unsigned short context_size;
	unsigned int open_flags;

	int (*parse)(void *ctx, struct apk_ctx *ac, int opt, const char *optarg);
	int (*main)(void *ctx, struct apk_ctx *ac, struct apk_string_array *args);
};

void apk_applet_register(struct apk_applet *);
struct apk_applet *apk_applet_find(const char *name);
void apk_applet_help(struct apk_applet *applet, struct apk_out *out);

#define APK_DEFINE_APPLET(x) \
__attribute__((constructor)) static void __register_##x(void) { apk_applet_register(&x); }
