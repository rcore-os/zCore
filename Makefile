# Makefile for top level of zCore

ARCH ?= x86_64
XTASK ?= 1
LOG ?= error
IFACE ?= eno1
GRAPHIC ?= on
ACCEL ?= 1
# Desktop session for `make qemu`: labwc — the session verified to reach a
# full desktop in QEMU (X still hangs in its /sys/class/drm platform probe;
# see the branch history). The kernel cmdline gets `desktop=$(DESKTOP)`, which
# eclipse-init reads to pick the session. Real hardware images carry no such
# argument and also default to labwc via /etc/eclipse/desktop. Override with
# `make qemu DESKTOP=xorg` to work on the X session.
DESKTOP ?= labwc

STRIP := $(ARCH)-linux-musl-strip
export PATH=$(shell printenv PATH):$(CURDIR)/ignored/target/$(ARCH)/$(ARCH)-linux-musl-cross/bin/

.PHONY: help zircon-init update rootfs libc-test other-test image check doc clean
.PHONY: iso qcow2 img

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
image: rootfs
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
qemu: image
	$(MAKE) -C zCore run MODE=release LINUX=1 LOG=$(LOG) GRAPHIC=$(GRAPHIC) ACCEL=$(ACCEL) DESKTOP=$(DESKTOP)

vbox: image
	$(MAKE) -C zCore vbox MODE=release LINUX=1 LOG=$(LOG) GRAPHIC=$(GRAPHIC) ACCEL=$(ACCEL) IFACE=$(IFACE)

# Macvtap networking: VM gets its own MAC/IP on the physical LAN.
# Useful for testing the I219-V driver on bare metal.
# Usage: make qemu-macvtap [LOG=warn] [IFACE=eno1]
qemu-real: image
	IFACE=$(IFACE) LOG=$(LOG) ACCEL=$(ACCEL) GRAPHIC=$(GRAPHIC) \
		bash zCore/run-qemu-macvtap.sh

################ Distribution images ################
#
# - `make iso`   builds a UEFI-bootable ISO image.
# - `make qcow2` builds a qcow2 disk image that contains the ESP filesystem.
#
# Both targets rely on the existing ESP directory produced by `zCore/Makefile`
# when building for x86_64.

DIST_DIR := $(CURDIR)/dist
BUILD_DIR := $(CURDIR)/build
MODE ?= release
ESP_DIR := $(CURDIR)/target/$(ARCH)/$(MODE)/esp

ISO_STAGING := $(BUILD_DIR)/iso-root
ESP_IMG := $(BUILD_DIR)/esp.img
DISK_IMG := $(BUILD_DIR)/disk.img
ESP_IMG_SIZE_MB ?= 1024
DISK_IMG_SIZE_MB ?= 1024
ISO_OUT := $(DIST_DIR)/eclipse-$(ARCH).iso
QCOW2_OUT := $(DIST_DIR)/eclipse-$(ARCH).qcow2
IMG_OUT := $(DIST_DIR)/eclipse-$(ARCH).img

iso: image
ifeq ($(ARCH), x86_64)
	@$(MAKE) -C zCore build MODE=release LINUX=1 LOG=$(LOG) GRAPHIC=on GL=$(GL) DESKTOP=$(DESKTOP)
	@mkdir -p "$(DIST_DIR)" "$(BUILD_DIR)" "$(ISO_STAGING)"
	@test -d "$(ESP_DIR)/EFI" || (echo "ESP no encontrado en $(ESP_DIR). ¿Has compilado zCore para x86_64?"; exit 1)
	@rm -rf "$(ISO_STAGING)/EFI" && cp -a "$(ESP_DIR)/EFI" "$(ISO_STAGING)/"
	@command -v mkfs.vfat >/dev/null || (echo "falta mkfs.vfat (paquete: dosfstools)"; exit 1)
	@command -v mcopy >/dev/null || (echo "falta mcopy (paquete: mtools)"; exit 1)
	@command -v mmd >/dev/null || (echo "falta mmd (paquete: mtools)"; exit 1)
	@command -v xorriso >/dev/null || (echo "falta xorriso"; exit 1)
	@rm -f "$(ESP_DIR)/EFI/zCore/x86_64.img" "$(ESP_DIR)/EFI/zCore/aarch64.img" \
		"$(ESP_DIR)/EFI/zCore/riscv64.img"
	@rm -f "$(ESP_IMG)"
	@esp_mb=$$(du -sm "$(ESP_DIR)/EFI" | cut -f1); esp_mb=$$((esp_mb + 96)); \
		[ "$$esp_mb" -ge "$(ESP_IMG_SIZE_MB)" ] || esp_mb=$(ESP_IMG_SIZE_MB); \
		echo "ESP: $$esp_mb MiB"; \
		dd if=/dev/zero of="$(ESP_IMG)" bs=1M count=$$esp_mb status=none
	@mkfs.vfat -F 32 "$(ESP_IMG)" >/dev/null
	@mmd -i "$(ESP_IMG)" ::/EFI ::/EFI/Boot ::/EFI/zCore >/dev/null
	@mcopy -i "$(ESP_IMG)" -s "$(ESP_DIR)/EFI" ::/ >/dev/null
	@mkdir -p "$(ISO_STAGING)/boot"
	@cp -f "$(ESP_IMG)" "$(ISO_STAGING)/boot/efi.img"
	@cp -f "ignored/target/efi.img.gz" "$(ISO_STAGING)/boot/efi.img.gz"
	@cp -f "ignored/target/rootfs.btrfs.gz" "$(ISO_STAGING)/boot/rootfs.btrfs.gz"
	@cp -f "ignored/target/home.btrfs.gz" "$(ISO_STAGING)/boot/home.btrfs.gz"
	@xorriso -as mkisofs \
		-R -J -V "ECLIPSE" \
		-eltorito-alt-boot \
		-e "boot/efi.img" \
		-no-emul-boot \
		-isohybrid-gpt-basdat \
		-append_partition 2 0xef "$(ESP_IMG)" \
		-o "$(ISO_OUT)" \
		"$(ISO_STAGING)" >/dev/null
	@echo "ISO generado: $(ISO_OUT)"
else
	@echo "iso: solo soportado para ARCH=x86_64 por ahora"
	@exit 1
endif

qcow2: image
ifeq ($(ARCH), x86_64)
	@mkdir -p "$(DIST_DIR)" "$(BUILD_DIR)"
	@test -d "$(ESP_DIR)/EFI" || (echo "ESP no encontrado en $(ESP_DIR). ¿Has compilado zCore para x86_64?"; exit 1)
	@command -v mkfs.vfat >/dev/null || (echo "falta mkfs.vfat (paquete: dosfstools)"; exit 1)
	@command -v mcopy >/dev/null || (echo "falta mcopy (paquete: mtools)"; exit 1)
	@command -v mmd >/dev/null || (echo "falta mmd (paquete: mtools)"; exit 1)
	@command -v qemu-img >/dev/null || (echo "falta qemu-img"; exit 1)
	@rm -f "$(ESP_DIR)/EFI/zCore/x86_64.img" "$(ESP_DIR)/EFI/zCore/aarch64.img" \
		"$(ESP_DIR)/EFI/zCore/riscv64.img"
	@rm -f "$(ESP_IMG)"
	@esp_mb=$$(du -sm "$(ESP_DIR)/EFI" | cut -f1); esp_mb=$$((esp_mb + 96)); \
		[ "$$esp_mb" -ge "$(ESP_IMG_SIZE_MB)" ] || esp_mb=$(ESP_IMG_SIZE_MB); \
		echo "ESP: $$esp_mb MiB"; \
		dd if=/dev/zero of="$(ESP_IMG)" bs=1M count=$$esp_mb status=none
	@mkfs.vfat -F 32 "$(ESP_IMG)" >/dev/null
	@mmd -i "$(ESP_IMG)" ::/EFI ::/EFI/Boot ::/EFI/zCore >/dev/null
	@mcopy -i "$(ESP_IMG)" -s "$(ESP_DIR)/EFI" ::/ >/dev/null
	@qemu-img convert -f raw "$(ESP_IMG)" -O qcow2 "$(QCOW2_OUT)" >/dev/null
	@echo "qcow2 generado: $(QCOW2_OUT)"
else
	@echo "qcow2: solo soportado para ARCH=x86_64 por ahora"
	@exit 1
endif

img: image
ifeq ($(ARCH), x86_64)
	@mkdir -p "$(DIST_DIR)" "$(BUILD_DIR)"
	@test -d "$(ESP_DIR)/EFI" || (echo "ESP no encontrado en $(ESP_DIR). ¿Has compilado zCore para x86_64?"; exit 1)
	@command -v sgdisk >/dev/null || (echo "falta sgdisk (paquete: gdisk)"; exit 1)
	@command -v mformat >/dev/null || (echo "falta mformat (paquete: mtools)"; exit 1)
	@command -v mcopy >/dev/null || (echo "falta mcopy (paquete: mtools)"; exit 1)
	@command -v mmd >/dev/null || (echo "falta mmd (paquete: mtools)"; exit 1)
	@rm -f "$(ESP_DIR)/EFI/zCore/x86_64.img" "$(ESP_DIR)/EFI/zCore/aarch64.img" \
		"$(ESP_DIR)/EFI/zCore/riscv64.img"
	@rm -f "$(DISK_IMG)"
	@esp_mb=$$(du -sm "$(ESP_DIR)/EFI" | cut -f1); esp_mb=$$((esp_mb + 96)); \
		[ "$$esp_mb" -ge "$(ESP_IMG_SIZE_MB)" ] || esp_mb=$(ESP_IMG_SIZE_MB); \
		disk_mb=$$((esp_mb + 8)); \
		echo "disk: $$disk_mb MiB (ESP $$esp_mb MiB)"; \
		dd if=/dev/zero of="$(DISK_IMG)" bs=1M count=$$disk_mb status=none; \
		sgdisk -o "$(DISK_IMG)" >/dev/null; \
		sgdisk -n 1:2048:+$${esp_mb}M -t 1:ef00 -c 1:EFI "$(DISK_IMG)" >/dev/null
	@# FAT32 ESP starts at 2048 * 512 = 1048576 bytes (1MiB)
	@mformat -i "$(DISK_IMG)@@1048576" -F -v EFI :: >/dev/null
	@mmd -i "$(DISK_IMG)@@1048576" ::/EFI ::/EFI/Boot ::/EFI/zCore >/dev/null
	@mcopy -i "$(DISK_IMG)@@1048576" -s "$(ESP_DIR)/EFI" ::/ >/dev/null
	@cp -f "$(DISK_IMG)" "$(IMG_OUT)"
	@echo "img generado: $(IMG_OUT)"
else
	@echo "img: solo soportado para ARCH=x86_64 por ahora"
	@exit 1
endif
