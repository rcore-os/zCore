# Makefile for top level of zCore

ARCH ?= x86_64
XTASK ?= 1

STRIP := $(ARCH)-linux-musl-strip
export PATH=$(shell printenv PATH):$(CURDIR)/ignored/target/$(ARCH)/$(ARCH)-linux-musl-cross/bin/

.PHONY: help zircon-init update rootfs libc-test other-test image check doc clean

# print top level help
help:
	cargo xtask

# download zircon binaries
zircon-init:
	cargo zircon-init

# update toolchain and dependencies
update:
	cargo update-all

# put rootfs for linux mode
rootfs:
ifeq ($(XTASK), 1)
	cargo rootfs --arch $(ARCH)
else ifeq ($(ARCH), riscv64)
	@rm -rf rootfs/riscv && mkdir -p rootfs/riscv/bin
	@wget https://github.com/rcore-os/busybox-prebuilts/raw/master/busybox-1.30.1-riscv64/busybox -O rootfs/riscv/bin/busybox
	@ln -s busybox rootfs/riscv/bin/ls
endif

# put libc tests into rootfs
libc-test:
	cargo libc-test --arch $(ARCH)
	find rootfs/$(ARCH)/libc-test -type f \
	       -name "*so" -o -name "*exe" -exec $(STRIP) {} \; 

# put other tests into rootfs
other-test:
	cargo other-test --arch $(ARCH)

# build image from rootfs
image:
ifeq ($(XTASK), 1)
	cargo image --arch $(ARCH)
else ifeq ($(ARCH), riscv64)
	@echo building riscv.img
	@rcore-fs-fuse zCore/riscv64.img rootfs/riscv zip
	@qemu-img resize -f raw zCore/riscv64.img +5M
endif

# check code style
check:
	cargo check-style

# build and open project document
doc:
	cargo doc --open

# clean targets
clean:
	cargo clean
	rm -f  *.asm
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
