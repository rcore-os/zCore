ARCH ?= x86_64

ROOTFS_TAR := alpine-minirootfs-3.12.0-$(ARCH).tar.gz
ROOTFS_URL := http://dl-cdn.alpinelinux.org/alpine/v3.12/releases/$(ARCH)/$(ROOTFS_TAR)

rcore_fs_fuse_revision := 7f5eeac
OUT_IMG := zCore/$(ARCH).img
TMP_ROOTFS := /tmp/rootfs

# for linux syscall tests
TEST_DIR := linux-syscall/test/
DEST_DIR := rootfs/bin/
TEST_PATH := $(wildcard $(TEST_DIR)*.c)
BASENAMES := $(notdir  $(basename $(TEST_PATH)))

CC := $(ARCH)-linux-musl-gcc
CFLAG := -W

.PHONY: rootfs libc-test rcore-fs-fuse image

prebuilt/linux/$(ROOTFS_TAR):
ifeq ($(ARCH), x86_64)
	wget $(ROOTFS_URL) -O $@
endif

rootfs: prebuilt/linux/$(ROOTFS_TAR)
	rm -rf rootfs && mkdir -p rootfs
ifeq ($(ARCH), x86_64)
	echo hhh
	echo $<
	tar -xf $< -C rootfs
	cp prebuilt/linux/libc-libos.so rootfs/lib/ld-musl-$(ARCH).so.1
	@for VAR in $(BASENAMES); do $(CC) $(TEST_DIR)$$VAR.c -o $(DEST_DIR)$$VAR $(CFLAG); done
endif

libc-test:
	cd rootfs && git clone git://repo.or.cz/libc-test --depth 1
	cd rootfs/libc-test && cp config.mak.def config.mak && echo 'CC := musl-gcc' >> config.mak && make -j

rcore-fs-fuse:
ifneq ($(shell rcore-fs-fuse dir image git-version), $(rcore_fs_fuse_revision))
	@echo Installing rcore-fs-fuse
	@cargo install rcore-fs-fuse --git https://github.com/rcore-os/rcore-fs --rev $(rcore_fs_fuse_revision) --force
endif

$(OUT_IMG): rootfs rcore-fs-fuse
	@echo Generating $(ARCH).img
ifeq ($(ARCH), $(filter $(ARCH), x86_64))
	@mkdir -p $(TMP_ROOTFS)
	@cp $(TMP_ROOTFS)/lib/ld-musl-$(ARCH).so.1 rootfs/lib/
endif
	@rcore-fs-fuse $@ rootfs zip

image: $(OUT_IMG)
	@echo Resizing $(ARCH).img
	@qemu-img resize $(OUT_IMG) +50M

