/* apk_fs.h - Alpine Package Keeper (APK)
 *
 * Copyright (C) 2021 Timo Teräs <timo.teras@iki.fi>
 * All rights reserved.
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once
#include "apk_context.h"
#include "apk_io.h"
#include "apk_pathbuilder.h"

#define APK_FS_PRIO_DISK	0
#define APK_FS_PRIO_UVOL	1
#define APK_FS_PRIO_MAX		2

#define APK_FS_CTRL_COMMIT		1
#define APK_FS_CTRL_APKNEW		2
#define APK_FS_CTRL_CANCEL		3
#define APK_FS_CTRL_DELETE		4
#define APK_FS_CTRL_DELETE_APKNEW	5

#define APK_FS_DIR_MODIFIED	1

struct apk_fsdir_ops;

struct apk_fsdir {
	struct apk_ctx *ac;
	const struct apk_fsdir_ops *ops;
	struct apk_pathbuilder pb;
	unsigned int extract_flags;
	apk_blob_t pkgctx;
};

struct apk_fsdir_ops {
	uint8_t priority;

	int (*dir_create)(struct apk_fsdir *, mode_t, uid_t, gid_t);
	int (*dir_delete)(struct apk_fsdir *);
	int (*dir_check)(struct apk_fsdir *, mode_t, uid_t, gid_t);
	int (*dir_update_perms)(struct apk_fsdir *, mode_t, uid_t, gid_t);

	int (*file_extract)(struct apk_ctx *, const struct apk_file_info *, struct apk_istream *, unsigned int, apk_blob_t);
	int (*file_control)(struct apk_fsdir *, apk_blob_t, int);
	int (*file_info)(struct apk_fsdir *, apk_blob_t, unsigned int, struct apk_file_info *);
};

#define APK_FSEXTRACTF_NO_CHOWN		0x0001
#define APK_FSEXTRACTF_NO_OVERWRITE	0x0002
#define APK_FSEXTRACTF_NO_SYS_XATTRS	0x0004
#define APK_FSEXTRACTF_NO_DEVICES	0x0008

int apk_fs_extract(struct apk_ctx *, const struct apk_file_info *, struct apk_istream *, unsigned int, apk_blob_t);

void apk_fsdir_get(struct apk_fsdir *, apk_blob_t dir, unsigned int extract_flags, struct apk_ctx *ac, apk_blob_t pkgctx);

static inline uint8_t apk_fsdir_priority(struct apk_fsdir *fs) {
	return fs->ops->priority;
}
static inline int apk_fsdir_create(struct apk_fsdir *fs, mode_t mode, uid_t uid, gid_t gid) {
	return fs->ops->dir_create(fs, mode, uid, gid);
}
static inline int apk_fsdir_delete(struct apk_fsdir *fs) {
	return fs->ops->dir_delete(fs);
}
static inline int apk_fsdir_check(struct apk_fsdir *fs, mode_t mode, uid_t uid, gid_t gid) {
	return fs->ops->dir_check(fs, mode, uid, gid);
}
static inline int apk_fsdir_update_perms(struct apk_fsdir *fs, mode_t mode, uid_t uid, gid_t gid) {
	return fs->ops->dir_update_perms(fs, mode, uid, gid);
}

static inline int apk_fsdir_file_control(struct apk_fsdir *fs, apk_blob_t filename, int ctrl) {
	return fs->ops->file_control(fs, filename, ctrl);
}
static inline int apk_fsdir_file_info(struct apk_fsdir *fs, apk_blob_t filename, unsigned int flags, struct apk_file_info *fi) {
	return fs->ops->file_info(fs, filename, flags, fi);
}
