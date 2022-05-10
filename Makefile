# Makefile for top level of zCore

ARCH ?= x86_64

.PHONY: help setup rootfs libc-test image test-image check doc clean

# print top level help
help:
	cargo xtask help

# setup git lfs and git submodules
setup:
	cargo setup

# update toolchain and dependencies
update:
	cargo update-all

# put rootfs for linux mode
rootfs:
	cargo rootfs $(ARCH)

# put libc-test into rootfs
libc-test:
	cargo libc-test $(ARCH)

# build image from rootfs
image:
	cargo image $(ARCH)

# build image with libc-test
test-image: libc-test image

# check code style
check:
	cargo check-style

# build and open project document
doc:
	cargo doc --open


riscv-image: rcore-fs-fuse riscv-rootfs toolchain
	@echo building riscv.img
	@cd riscv_rootfs && mv libc-test libc-test-prebuild
	@cd riscv_rootfs &&  git clone $(LIBC_TEST_URL) --depth 1
	@cd riscv_rootfs/libc-test && cp config.mak.def config.mak && make ARCH=riscv64 CROSS_COMPILE=riscv64-linux-musl- -j
	@cd riscv_rootfs && cp libc-test-prebuild/functional/tls_align-static.exe libc-test/src/functional/
	@rcore-fs-fuse zCore/riscv64.img riscv_rootfs zip
	@qemu-img resize -f raw zCore/riscv64.img +5M

aarch64-image: rcore-fs-fuse aarch64-rootfs
	@echo building aarch64.img
	@rcore-fs-fuse zCore/aarch64.img aarch64_rootfs zip
	@qemu-img resize -f raw zCore/aarch64.img +5M

clean:
	cargo clean
	find zCore -maxdepth 1 -name "*.img" -delete
	rm -rf rootfs
	rm -rf riscv-rootfs
	rm -rf aarch64_rootfs
	find zCore/target -type f -name "*.zbi" -delete
	find zCore/target -type f -name "*.elf" -delete

rt-test:
	cd rootfs && git clone https://kernel.googlesource.com/pub/scm/linux/kernel/git/clrkwllms/rt-tests --depth 1
	cd rootfs/rt-tests && make
	echo x86 gcc build rt-test,now need manual modificy.
