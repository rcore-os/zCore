# Makefile for top level of zCore

ARCH ?= x86_64

.PHONY: help setup update rootfs libc-test other-test image check doc clean

# print top level help
help:
	cargo xtask

# setup git lfs and git submodules
setup:
	cargo initialize

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

# build and open project document
doc:
	cargo doc --open

# clean targets
clean:
	cargo clean
	rm -rf rootfs
	rm -rf zCore/disk
	find zCore -maxdepth 1 -name "*.img" -delete
	find zCore -maxdepth 1 -name "*.bin" -delete

# delete targets, including those that are large and compile slowly
cleanup: clean
	rm -rf ignored/target

# delete everything, including origin files that are downloaded directly
clean-everything: clean
	rm -rf ignored

# rt-test:
# 	cd rootfs/x86_64 && git clone https://kernel.googlesource.com/pub/scm/linux/kernel/git/clrkwllms/rt-tests --depth 1
# 	cd rootfs/x86_64/rt-tests && make
# 	echo x86 gcc build rt-test,now need manual modificy.
