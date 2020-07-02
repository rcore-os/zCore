ROOTFS_TAR := alpine-minirootfs-3.12.0-x86_64.tar.gz
ROOTFS_URL := http://dl-cdn.alpinelinux.org/alpine/v3.12/releases/x86_64/$(ROOTFS_TAR)

.PHONY: rootfs

prebuilt/linux/$(ROOTFS_TAR):
	wget $(ROOTFS_URL) -O $@

rootfs: prebuilt/linux/$(ROOTFS_TAR)
	rm -rf rootfs && mkdir -p rootfs
	tar xf $< -C rootfs
	cp prebuilt/linux/libc-libos.so rootfs/lib/ld-musl-x86_64.so.1
