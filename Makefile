# Makefile for top level of zCore

PATH := $(PATH):$(PWD)/toolchain/riscv64-linux-musl-cross/bin
ARCH ?= x86_64

.PHONY: rootfs libc-test image test-image check doc clean

rootfs:
	cargo rootfs $(ARCH)

libc-test:
	cargo libc-test $(ARCH)

image: rootfs
	cargo image $(ARCH)

test-image: rootfs libc-test image

check:
	cargo xtask check

doc:
	cargo doc --open

clean:
	cargo clean
	find zCore -maxdepth 1 -name "*.img" -delete
	rm -rf rootfs
	rm -rf riscv_rootfs
	rm -rf toolchain
	find zCore/target -type f -name "*.zbi" -delete
	find zCore/target -type f -name "*.elf" -delete

rt-test:
	cd rootfs && git clone https://kernel.googlesource.com/pub/scm/linux/kernel/git/clrkwllms/rt-tests --depth 1
	cd rootfs/rt-tests && make
	echo x86 gcc build rt-test,now need manual modificy.
