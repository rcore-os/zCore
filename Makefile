ROOTFS_TAR := alpine-minirootfs-3.12.0-x86_64.tar.gz
ROOTFS_URL := http://dl-cdn.alpinelinux.org/alpine/v3.12/releases/x86_64/$(ROOTFS_TAR)

# for linux syscall tests
TEST_DIR := linux-syscall/test/
DEST_DIR := rootfs/bin/
TEST_PATH := $(wildcard $(TEST_DIR)*.c)
BASENAMES := $(notdir  $(basename $(TEST_PATH)))

CFLAG := -Wl,--dynamic-linker=/lib/ld-musl-x86_64.so.1

.PHONY: rootfs

prebuilt/linux/$(ROOTFS_TAR):
	wget $(ROOTFS_URL) -O $@

rootfs: prebuilt/linux/$(ROOTFS_TAR)
	rm -rf rootfs && mkdir -p rootfs
	tar xf $< -C rootfs
	cp prebuilt/linux/libc-libos.so rootfs/lib/ld-musl-x86_64.so.1
	@for VAR in $(BASENAMES); do gcc $(TEST_DIR)$$VAR.c -o $(DEST_DIR)$$VAR $(CFLAG); done