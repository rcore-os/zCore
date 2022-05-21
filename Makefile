# Makefile for top level of zCore

ARCH ?= x86_64

.PHONY: help setup update rootfs libc-test other-test image check doc clean

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
	cargo rootfs --arch $(ARCH)

# put libc tests into rootfs
libc-test:
	cargo libc-test --arch $(ARCH)

# put other tests into rootfs
other-test:
	cargo other-test --arch $(ARCH)

# build image from rootfs
image:
	cargo image --arch $(ARCH)

# check code style
check:
	cargo check-style

# build and open project document
doc:
	cargo doc --open

aarch64-image: rcore-fs-fuse aarch64-rootfs
	@echo building aarch64.img
	@rcore-fs-fuse zCore/aarch64.img aarch64_rootfs zip
	@qemu-img resize -f raw zCore/aarch64.img +5M

# clean targets
clean:
	cargo clean
	rm -rf rootfs
	rm -rf ignored/target
	find zCore -maxdepth 1 -name "*.img" -delete

rt-test:
	cd rootfs/x86_64 && git clone https://kernel.googlesource.com/pub/scm/linux/kernel/git/clrkwllms/rt-tests --depth 1
	cd rootfs/x86_64/rt-tests && make
	echo x86 gcc build rt-test,now need manual modificy.
