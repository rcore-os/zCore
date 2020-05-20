arch ?= x86_64
mode ?= debug
LOG ?=
zbi_file ?= bringup
graphic ?=
accel ?=

build_args := -Z build-std=core,alloc --target $(arch).json
build_path := target/$(arch)/$(mode)
kernel := $(build_path)/zcore
kernel_img := $(build_path)/zcore.img
ESP := $(build_path)/esp
OVMF := ../rboot/OVMF.fd
TOOLS_PATH := ../prebuilt/tools
qemu := /media/dflasher/Large/fuchsia/prebuilt/third_party/qemu/linux-x64/bin/qemu-system-x86_64
OBJDUMP := rust-objdump
VMDISK := $(build_path)/boot.vdi

ifeq ($(mode), release)
	build_args += --release
endif

qemu_opts := \
	-smp 4,threads=2

ifeq ($(arch), x86_64)
qemu_opts += \
	-cpu Haswell,+smap,-check,-fsgsbase \
	-bios $(OVMF) \
	-drive format=raw,file=fat:rw:$(ESP) \
	-serial mon:stdio \
	-m 4G \
	-nic none \
	-device isa-debug-exit,iobase=0xf4,iosize=0x04
endif

ifeq ($(accel), 1)
ifeq ($(shell uname), Darwin)
qemu_opts += -accel hax
else
qemu_opts += -accel kvm -cpu host,migratable=no,+invtsc
endif
endif

ifeq ($(graphic), on)
build_args += --features graphic
else
qemu_opts += -display none -nographic
endif

run: build justrun

debug: build debugrun

debugrun:
	$(qemu) $(qemu_opts) -s -S

justrun:
	$(qemu) $(qemu_opts)

build: $(kernel_img)

$(kernel_img): kernel bootloader
	mkdir -p $(ESP)/EFI/zCore $(ESP)/EFI/boot
	cp ../rboot/target/x86_64-unknown-uefi/release/rboot.efi $(ESP)/EFI/boot/bootx64.efi
	cp rboot.conf $(ESP)/EFI/boot/rboot.conf
	cp ../prebuilt/zircon/$(zbi_file).zbi $(ESP)/EFI/zCore/fuchsia.zbi
	cp $(kernel) $(ESP)/EFI/zCore/zcore.elf
	echo \EFI\boot\bootx64.efi > $(ESP)/startup.nsh

kernel:
	echo Building zCore kenel
	cargo build $(build_args)

bootloader:
	cd ../rboot && make build

clean:
	cargo clean

image:
	# for macOS only
	hdiutil create -fs fat32 -ov -volname EFI -format UDTO -srcfolder $(ESP) $(build_path)/zcore.cdr
	qemu-img convert -f raw $(build_path)/zcore.cdr -O qcow2 $(build_path)/zcore.qcow2

header:
	$(OBJDUMP) -x $(kernel) | less

asm:
	$(OBJDUMP) -d $(kernel) | less

vbox:
ifneq "$(VMDISK)" "$(wildcard $(VMDISK))"
	vboxmanage createvm --name zCoreVM --basefolder $(build_path) --register
	cp ../prebuilt/zircon/empty.vdi $(VMDISK)
	vboxmanage storagectl zCoreVM --name DiskCtrlr --controller IntelAhci --add sata 
	vboxmanage storageattach zCoreVM --storagectl DiskCtrlr --port 0 --type hdd --medium $(VMDISK)
	vboxmanage modifyvm zCoreVM --memory 1024 --firmware efi
	tar -cvf $(build_path)/esp.tar -C $(build_path)/esp EFI
	sudo LIBGUESTFS_DEBUG=1 guestfish -a $(VMDISK) -m /dev/sda1 tar-in $(build_path)/esp.tar / : quit
endif
	# depencency: libguestfs-tools
	tar -cvf $(build_path)/esp.tar -C $(build_path)/esp EFI
	sudo guestfish -a $(VMDISK) -m /dev/sda1 tar-in $(build_path)/esp.tar / : quit
	# sudo LIBGUESTFS_DEBUG=1 guestfish -a $(VMDISK) -m /dev/sda1 tar-in $(build_path)/esp.tar / : quit
	# -a $(VMDISK) $(build_path)/esp.tar /
	rm $(build_path)/esp.tar
	vboxmanage startvm zCoreVM 
