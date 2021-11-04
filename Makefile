ROOTFS_TAR := alpine-minirootfs-3.12.0-x86_64.tar.gz
ROOTFS_URL := http://dl-cdn.alpinelinux.org/alpine/v3.12/releases/x86_64/$(ROOTFS_TAR)

RISCV64_ROOTFS_TAR := prebuild.tar.xz
RISCV64_ROOTFS_URL := https://github.com/rcore-os/libc-test-prebuilt/releases/download/0.1/$(RISCV64_ROOTFS_TAR)

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

prebuilt/linux/riscv64/$(RISCV64_ROOTFS_TAR):
	@wget $(RISCV64_ROOTFS_URL) -O $@

rootfs: prebuilt/linux/$(ROOTFS_TAR)
	rm -rf rootfs && mkdir -p rootfs
	tar xf $< -C rootfs
# libc-libos.so (convert syscall to function call) is from https://github.com/rcore-os/musl/tree/rcore
	cp prebuilt/linux/libc-libos.so rootfs/lib/ld-musl-x86_64.so.1
	@for VAR in $(BASENAMES); do gcc $(TEST_DIR)$$VAR.c -o $(DEST_DIR)$$VAR $(CFLAG); done

riscv-rootfs:prebuilt/linux/riscv64/$(RISCV64_ROOTFS_TAR)
	@rm -rf riscv_rootfs && mkdir -p riscv_rootfs
	@tar -xvf $< -C riscv_rootfs --strip-components 1
	@ln -s busybox riscv_rootfs/bin/ls

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
	@qemu-img resize $(OUT_IMG) +5M


riscv-image: rcore-fs-fuse riscv-rootfs
	@echo building riscv.img
	@rcore-fs-fuse zCore/riscv64.img riscv_rootfs zip
	@qemu-img resize -f raw zCore/riscv64.img +5M

clean:
	cargo clean
	find zCore -maxdepth 1 -name "*.img" -delete
	rm -rf rootfs
	rm -rf riscv-rootfs
	find zCore/target -type f -name "*.zbi" -delete
	find zCore/target -type f -name "*.elf" -delete
	cd linux-syscall/test-oscomp && make clean
	cd linux-syscall/busybox && make clean
	cd linux-syscall/lua && make clean
	cd linux-syscall/lmbench && make clean

doc:
	cargo doc --open

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
	@qemu-img resize $(OUT_IMG) +5M

baremetal-test:
	@make -C zCore baremetal-test MODE=release LINUX=1 | tee stdout-baremetal-test

baremetal-test-rv64:
	@make -C zCore baremetal-test-rv64 ARCH=riscv64 MODE=release LINUX=1 ROOTPROC=$(ROOTPROC) | tee -a stdout-baremetal-test-rv64 | tee stdout-rv64
