ROOTFS_TAR := alpine-minirootfs-3.11.3-x86_64.tar.gz
ROOTFS_URL := http://dl-cdn.alpinelinux.org/alpine/v3.11/releases/x86_64/$(ROOTFS_TAR)

.PHONY: rootfs

prebuilt/$(ROOTFS_TAR):
	wget $(ROOTFS_URL) -O $@

rootfs: prebuilt/$(ROOTFS_TAR)
	mkdir -p rootfs
	tar xf $< -C rootfs
	cp prebuilt/libc.so rootfs/lib/ld-musl-x86_64.so.1
