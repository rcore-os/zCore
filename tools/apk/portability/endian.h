/* endian.h - portable endian routines
 *
 * Copyright (C) 2005-2008 Natanael Copa <n@tanael.org>
 * Copyright (C) 2008-2011 Timo Teräs <timo.teras@iki.fi>
 * Copyright (C) 2011 Rich Felker
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#ifdef __linux__
# include_next <endian.h>
#else

#pragma once
#include <stdint.h>

static __inline uint16_t __portable_bswap16(uint16_t __x)
{
	return (__x<<8) | (__x>>8);
}

static __inline uint32_t __portable_bswap32(uint32_t __x)
{
	return (__x>>24) | (__x>>8&0xff00) | (__x<<8&0xff0000) | (__x<<24);
}

static __inline uint64_t __portable_bswap64(uint64_t __x)
{
	return (__portable_bswap32(__x)+0ULL)<<32 | __portable_bswap32(__x>>32);
}

#if __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
# define htobe16(x) __portable_bswap16(x)
# define be16toh(x) __portable_bswap16(x)
# define htobe32(x) __portable_bswap32(x)
# define be32toh(x) __portable_bswap32(x)
# define htobe64(x) __portable_bswap64(x)
# define be64toh(x) __portable_bswap64(x)
# define htole16(x) (uint16_t)(x)
# define le16toh(x) (uint16_t)(x)
# define htole32(x) (uint32_t)(x)
# define le32toh(x) (uint32_t)(x)
# define htole64(x) (uint64_t)(x)
# define le64toh(x) (uint64_t)(x)
#else
# define htobe16(x) (uint16_t)(x)
# define be16toh(x) (uint16_t)(x)
# define htobe32(x) (uint32_t)(x)
# define be32toh(x) (uint32_t)(x)
# define htobe64(x) (uint64_t)(x)
# define be64toh(x) (uint64_t)(x)
# define htole16(x) __portable_bswap16(x)
# define le16toh(x) __portable_bswap16(x)
# define htole32(x) __portable_bswap32(x)
# define le32toh(x) __portable_bswap32(x)
# define htole64(x) __portable_bswap64(x)
# define le64toh(x) __portable_bswap64(x)
#endif

#endif
