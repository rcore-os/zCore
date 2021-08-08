ROOTFS_TAR := alpine-minirootfs-3.12.0-x86_64.tar.gz
ROOTFS_URL := http://dl-cdn.alpinelinux.org/alpine/v3.12/releases/x86_64/$(ROOTFS_TAR)

ARCH ?= x86_64
rcore_fs_fuse_revision := 7f5eeac
OUT_IMG := zCore/$(ARCH).img
TMP_ROOTFS := /tmp/rootfs

# for linux syscall tests
TEST_DIR := linux-syscall/test/
DEST_DIR := rootfs/bin/
TEST_PATH := $(wildcard $(TEST_DIR)*.c)
BASENAMES := $(notdir  $(basename $(TEST_PATH)))

CFLAG := -Wl,--dynamic-linker=/lib/ld-musl-x86_64.so.1

.PHONY: rootfs libc-test rcore-fs-fuse image

prebuilt/linux/$(ROOTFS_TAR):
	wget $(ROOTFS_URL) -O $@

rootfs: prebuilt/linux/$(ROOTFS_TAR)
	rm -rf rootfs && mkdir -p rootfs
	tar xf $< -C rootfs
# libc-libos.so (convert syscall to function call) is from https://github.com/rcore-os/musl/tree/rcore
	cp prebuilt/linux/libc-libos.so rootfs/lib/ld-musl-x86_64.so.1
	@for VAR in $(BASENAMES); do gcc $(TEST_DIR)$$VAR.c -o $(DEST_DIR)$$VAR $(CFLAG); done

libc-test:
	cd rootfs && git clone git://repo.or.cz/libc-test --depth 1
	cd rootfs/libc-test && cp config.mak.def config.mak && echo 'CC := musl-gcc' >> config.mak && make -j

rcore-fs-fuse:
ifneq ($(shell rcore-fs-fuse dir image git-version), $(rcore_fs_fuse_revision))
	@echo Installing rcore-fs-fuse
	@cargo install rcore-fs-fuse --git https://github.com/rcore-os/rcore-fs --rev $(rcore_fs_fuse_revision) --force
endif

$(OUT_IMG): prebuilt/linux/$(ROOTFS_TAR) rcore-fs-fuse
	@echo Generating $(ARCH).img
	@rm -rf $(TMP_ROOTFS)
	@mkdir -p $(TMP_ROOTFS)
	@tar xf $< -C $(TMP_ROOTFS)
	@cp $(TMP_ROOTFS)/lib/ld-musl-x86_64.so.1 rootfs/lib/
	@rcore-fs-fuse $@ rootfs zip
# recover rootfs/ld-musl-x86_64.so.1 for zcore usr libos
# libc-libos.so (convert syscall to function call) is from https://github.com/rcore-os/musl/tree/rcore
	@cp prebuilt/linux/libc-libos.so rootfs/lib/ld-musl-x86_64.so.1

image: $(OUT_IMG)
	@echo Resizing $(ARCH).img
	@qemu-img resize $(OUT_IMG) +50M


rv64-image: rcore-fs-fuse
	@echo building riscv64.img
	@rm -rf rootfs
	@mkdir rootfs
	@mkdir rootfs/bin
ifeq ($(wildcard prebuilt/linux/riscv64/busybox),)
	@mkdir -p prebuilt/linux/riscv64
	@wget https://github.com/rcore-os/busybox-prebuilts/raw/master/busybox-1.30.1-riscv64/busybox -O prebuilt/linux/riscv64/busybox
endif
	@cp prebuilt/linux/riscv64/* rootfs/bin/
	@@rcore-fs-fuse riscv64.img rootfs zip
	@qemu-img resize riscv64.img +50M

clean:
	cargo clean

doc:
	arch=x86_64 cargo doc --open

baremetal-test-img: prebuilt/linux/$(ROOTFS_TAR) rcore-fs-fuse
	@echo Generating $(ARCH).img
	@rm -rf $(TMP_ROOTFS)
	@mkdir -p $(TMP_ROOTFS)
	@tar xf $< -C $(TMP_ROOTFS)
	@mkdir -p rootfs/lib
	@cp $(TMP_ROOTFS)/lib/ld-musl-x86_64.so.1 rootfs/lib/
	@cd rootfs && rm -rf libc-test && git clone git://repo.or.cz/libc-test --depth 1
	@cd rootfs/libc-test && cp config.mak.def config.mak && echo 'CC := musl-gcc' >> config.mak && make -j
	@rcore-fs-fuse $(OUT_IMG) rootfs zip
# recover rootfs/ld-musl-x86_64.so.1 for zcore usr libos
# libc-libos.so (convert syscall to function call) is from https://github.com/rcore-os/musl/tree/rcore
	@cp prebuilt/linux/libc-libos.so rootfs/lib/ld-musl-x86_64.so.1
	@echo Resizing $(ARCH).img
	@qemu-img resize $(OUT_IMG) +50M

baremetal-test:
	@make -C zCore baremetal-test mode=release linux=1 | tee stdout-baremetal-test