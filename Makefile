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

clean:
	cargo clean
	rm -rf rootfs
	rm -rf ignored/target
	find zCore -maxdepth 1 -name "*.img" -delete

rt-test:
	cd rootfs/x86_64 && git clone https://kernel.googlesource.com/pub/scm/linux/kernel/git/clrkwllms/rt-tests --depth 1
	cd rootfs/x86_64/rt-tests && make
	echo x86 gcc build rt-test,now need manual modificy.
