/* version.c - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <ctype.h>

#include "apk_defines.h"
#include "apk_version.h"
#include "apk_ctype.h"

//#define DEBUG_PRINT

#ifdef DEBUG_PRINT
#include <stdio.h>
#define dbg_printf(args...) fprintf(stderr, args)
#else
#define dbg_printf(args...)
#endif

/* Alpine version: digit{.digit}...{letter}{_suf{#}}...{~hash}{-r#} */

enum PARTS {
	TOKEN_INITIAL_DIGIT,
	TOKEN_DIGIT,
	TOKEN_LETTER,
	TOKEN_SUFFIX,
	TOKEN_SUFFIX_NO,
	TOKEN_COMMIT_HASH,
	TOKEN_REVISION_NO,
	TOKEN_END,
	TOKEN_INVALID,
};

#define DECLARE_SUFFIXES(func) \
	func(INVALID, "") \
	func(ALPHA, "alpha") \
	func(BETA, "beta") \
	func(PRE, "pre") \
	func(RC, "rc") \
	func(NONE, "") \
	func(CVS, "cvs") \
	func(SVN, "svn") \
	func(GIT, "git") \
	func(HG, "hg") \
	func(P, "p")

#define SUFFIX_ENUM(n, str) SUFFIX_##n,
enum {
	SUFFIX_ENUM_START=-1,
	DECLARE_SUFFIXES(SUFFIX_ENUM)
};

struct token_state {
	unsigned int token;
	unsigned int suffix;
	uint64_t number;
	apk_blob_t value;
};

static int suffix_value(apk_blob_t suf)
{
#define SUFFIX_DEFINE(n, str) char suffix_##n[sizeof(str)];
#define SUFFIX_ASSIGN(n, str) str,
#define SUFFIX_INDEX(n, str) [SUFFIX_##n] = offsetof(struct suffix_literals, suffix_##n),
	static const struct suffix_literals {
		DECLARE_SUFFIXES(SUFFIX_DEFINE)
	} suffixes = {
		DECLARE_SUFFIXES(SUFFIX_ASSIGN)
	};
	static const unsigned short suffix_indexes[] = {
		DECLARE_SUFFIXES(SUFFIX_INDEX)
		sizeof(suffixes)
	};
	int val;

	if (suf.len == 0) return SUFFIX_INVALID;
	switch (suf.ptr[0]) {
	case 'a': val = SUFFIX_ALPHA; break;
	case 'b': val = SUFFIX_BETA; break;
	case 'c': val = SUFFIX_CVS; break;
	case 'g': val = SUFFIX_GIT; break;
	case 'h': val = SUFFIX_HG; break;
	case 'p': val = suf.len > 1 ? SUFFIX_PRE : SUFFIX_P; break;
	case 'r': val = SUFFIX_RC; break;
	case 's': val = SUFFIX_SVN; break;
	default: return SUFFIX_INVALID;
	}
	char *ptr = (char *)&suffixes + suffix_indexes[val];
	unsigned short len = suffix_indexes[val+1] - suffix_indexes[val] - 1;
	if (apk_blob_compare(suf, APK_BLOB_PTR_LEN(ptr, len)) != 0)
		return SUFFIX_INVALID;
	return val;
}

static int token_cmp(struct token_state *ta, struct token_state *tb)
{
	uint64_t a, b;
	int r;

	switch (ta->token) {
	case TOKEN_DIGIT:
		if (ta->value.ptr[0] == '0' || tb->value.ptr[0] == '0') {
			// if either of the digits have a leading zero, use
			// raw string comparison similar to Gentoo spec
			goto use_string_sort;
		}
		// fall throught to numeric comparison
	case TOKEN_INITIAL_DIGIT:
	case TOKEN_SUFFIX_NO:
	case TOKEN_REVISION_NO:
		a = ta->number;
		b = tb->number;
		break;
	case TOKEN_LETTER:
		a = ta->value.ptr[0];
		b = tb->value.ptr[0];
		break;
	case TOKEN_SUFFIX:
		a = ta->suffix;
		b = tb->suffix;
		break;
	use_string_sort:
	default:
		r = apk_blob_sort(ta->value, tb->value);
		if (r < 0) return APK_VERSION_LESS;
		if (r > 0) return APK_VERSION_GREATER;
		return APK_VERSION_EQUAL;
	}
	if (a < b) return APK_VERSION_LESS;
	if (a > b) return APK_VERSION_GREATER;
	return APK_VERSION_EQUAL;
}

static void token_parse_digits(struct token_state *t, apk_blob_t *b)
{
	char *start = b->ptr;
	t->number = apk_blob_pull_uint(b, 10);
	t->value = APK_BLOB_PTR_LEN(start, b->ptr - start);
	if (t->value.len == 0) t->token = TOKEN_INVALID;
}

static void token_first(struct token_state *t, apk_blob_t *b)
{
	t->token = TOKEN_INITIAL_DIGIT;
	token_parse_digits(t, b);
}

static void token_next(struct token_state *t, apk_blob_t *b)
{
	if (b->len == 0) {
		t->token = TOKEN_END;
		return;
	}
	// determine the token type from the first letter and parse
	// the content just as a blob. validate also that the previous
	// token allows the subsequent token.
	switch (b->ptr[0]) {
	case 'a' ... 'z':
		if (t->token > TOKEN_DIGIT) goto invalid;
		t->value = APK_BLOB_PTR_LEN(b->ptr, 1);
		t->token = TOKEN_LETTER;
		b->ptr++, b->len--;
		break;
	case '.':
		if (t->token > TOKEN_DIGIT) goto invalid;
		b->ptr++, b->len--;
		// fallthrough to parse number
	case '0' ... '9':
		switch (t->token) {
		case TOKEN_INITIAL_DIGIT:
		case TOKEN_DIGIT:
			t->token = TOKEN_DIGIT;
			break;
		case TOKEN_SUFFIX:
			t->token = TOKEN_SUFFIX_NO;
			break;
		default:
			goto invalid;
		}
		token_parse_digits(t, b);
		break;
	case '_':
		if (t->token > TOKEN_SUFFIX_NO) goto invalid;
		b->ptr++, b->len--;
		apk_blob_spn(*b, APK_CTYPE_VERSION_SUFFIX, &t->value, b);
		t->suffix = suffix_value(t->value);
		if (t->suffix == SUFFIX_INVALID) goto invalid;
		t->token = TOKEN_SUFFIX;
		break;
	case '~':
		if (t->token >= TOKEN_COMMIT_HASH) goto invalid;
		b->ptr++, b->len--;
		apk_blob_spn(*b, APK_CTYPE_HEXDIGIT, &t->value, b);
		if (t->value.len == 0) goto invalid;
		t->token = TOKEN_COMMIT_HASH;
		break;
	case '-':
		if (t->token >= TOKEN_REVISION_NO) goto invalid;
		if (!apk_blob_pull_blob_match(b, APK_BLOB_STRLIT("-r"))) goto invalid;
		t->token = TOKEN_REVISION_NO;
		token_parse_digits(t, b);
		break;
	invalid:
	default:
		t->token = TOKEN_INVALID;
		break;
	}
}

const char *apk_version_op_string(int op)
{
	switch (op & ~APK_VERSION_CONFLICT) {
	case APK_VERSION_LESS:
		return "<";
	case APK_VERSION_LESS|APK_VERSION_EQUAL:
		return "<=";
	case APK_VERSION_LESS|APK_VERSION_EQUAL|APK_VERSION_FUZZY:
		return "<~";
	case APK_VERSION_EQUAL|APK_VERSION_FUZZY:
	case APK_VERSION_FUZZY:
		return "~";
	case APK_VERSION_EQUAL:
		return "=";
	case APK_VERSION_GREATER|APK_VERSION_EQUAL:
		return ">=";
	case APK_VERSION_GREATER|APK_VERSION_EQUAL|APK_VERSION_FUZZY:
		return ">~";
	case APK_VERSION_GREATER:
		return ">";
	case APK_DEPMASK_CHECKSUM:
		return "><";
	case APK_DEPMASK_ANY:
		return "";
	default:
		return "?";
	}
}

int apk_version_result_mask_blob(apk_blob_t op)
{
	int i, r = 0;
	for (i = 0; i < op.len; i++) {
		switch (op.ptr[i]) {
		case '<':
			r |= APK_VERSION_LESS;
			break;
		case '>':
			r |= APK_VERSION_GREATER;
			break;
		case '=':
			r |= APK_VERSION_EQUAL;
			break;
		case '~':
			r |= APK_VERSION_FUZZY|APK_VERSION_EQUAL;
			break;
		default:
			return 0;
		}
	}
	return r;
}

int apk_version_result_mask(const char *op)
{
	return apk_version_result_mask_blob(APK_BLOB_STR(op));
}

int apk_version_validate(apk_blob_t ver)
{
	struct token_state t;
	for (token_first(&t, &ver); t.token < TOKEN_END; token_next(&t, &ver))
		;
	return t.token == TOKEN_END;
}

static int apk_version_compare_fuzzy(apk_blob_t a, apk_blob_t b, bool fuzzy)
{
	struct token_state ta, tb;

	if (APK_BLOB_IS_NULL(a) || APK_BLOB_IS_NULL(b)) {
		if (APK_BLOB_IS_NULL(a) && APK_BLOB_IS_NULL(b))
			return APK_VERSION_EQUAL;
		return APK_VERSION_EQUAL | APK_VERSION_GREATER | APK_VERSION_LESS;
	}

	for (token_first(&ta, &a), token_first(&tb, &b);
	     ta.token == tb.token && ta.token < TOKEN_END;
	     token_next(&ta, &a), token_next(&tb, &b)) {
		int r = token_cmp(&ta, &tb);
		dbg_printf("at=%d <" BLOB_FMT "> bt=%d <" BLOB_FMT "> -> %d\n",
			ta.token, BLOB_PRINTF(ta.value),
			tb.token, BLOB_PRINTF(tb.value), r);
		if (r != APK_VERSION_EQUAL) return r;
	}
	dbg_printf("at=%d <" BLOB_FMT "> bt=%d <" BLOB_FMT ">\n",
		ta.token, BLOB_PRINTF(ta.value),
		tb.token, BLOB_PRINTF(tb.value));

	/* both have TOKEN_END or TOKEN_INVALID next? or fuzzy matching the prefix*/
	if (ta.token == tb.token) return APK_VERSION_EQUAL;
	if (tb.token == TOKEN_END && fuzzy) return APK_VERSION_EQUAL;

	/* leading version components and their values are equal,
	 * now the non-terminating version is greater unless it's a suffix
	 * indicating pre-release */
	if (ta.token == TOKEN_SUFFIX && ta.suffix < SUFFIX_NONE) return APK_VERSION_LESS;
	if (tb.token == TOKEN_SUFFIX && tb.suffix < SUFFIX_NONE) return APK_VERSION_GREATER;
	if (ta.token > tb.token) return APK_VERSION_LESS;
	if (tb.token > ta.token) return APK_VERSION_GREATER;
	return APK_VERSION_EQUAL;
}

int apk_version_compare(apk_blob_t a, apk_blob_t b)
{
	return apk_version_compare_fuzzy(a, b, false);
}

int apk_version_match(apk_blob_t a, int op, apk_blob_t b)
{
	int ok = 0;
	if ((op & APK_DEPMASK_ANY) == APK_DEPMASK_ANY ||
	    apk_version_compare_fuzzy(a, b, (op & APK_VERSION_FUZZY) ? true : false) & op) ok = 1;
	if (op & APK_VERSION_CONFLICT) ok = !ok;
	return ok;
}
