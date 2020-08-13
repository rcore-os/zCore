arch ?= x86_64
mode ?= debug
zbi_file ?= bringup
graphic ?=
accel ?=
linux ?=
user ?=
hypervisor ?=
smp ?= 1
test_filter ?= *.*

build_args := -Z build-std=core,alloc --target $(arch).json
build_path := target/$(arch)/$(mode)
kernel := $(build_path)/zcore
kernel_img := $(build_path)/zcore.img
ESP := $(build_path)/esp
OVMF := ../rboot/OVMF.fd
qemu := qemu-system-x86_64
OBJDUMP := rust-objdump -print-imm-hex -x86-asm-syntax=intel
VMDISK := $(build_path)/boot.vdi
QEMU_DISK := $(build_path)/disk.qcow2

ifeq ($(mode), release)
	build_args += --release
endif

ifeq ($(linux), 1)
	build_args += --features linux
else
	build_args += --features zircon
endif

qemu_opts := \
	-smp $(smp)

ifeq ($(arch), x86_64)
qemu_opts += \
	-machine q35 \
	-cpu Haswell,+smap,-check,-fsgsbase \
	-drive if=pflash,format=raw,readonly,file=$(OVMF) \
	-drive format=raw,file=fat:rw:$(ESP) \
	-drive format=qcow2,file=$(QEMU_DISK),id=disk,if=none \
	-device ich9-ahci,id=ahci \
	-device ide-drive,drive=disk,bus=ahci.0 \
	-serial mon:stdio \
	-m 4G \
	-nic none \
	-device isa-debug-exit,iobase=0xf4,iosize=0x04
endif

ifeq ($(hypervisor), 1)
build_args += --features hypervisor
accel = 1
endif

ifeq ($(accel), 1)
ifeq ($(shell uname), Darwin)
qemu_opts += -accel hvf
else
qemu_opts += -accel kvm -cpu host,migratable=no,+invtsc
endif
endif

ifeq ($(graphic), on)
build_args += --features graphic
else
ifeq ($(MAKECMDGOALS), vbox)
build_args += --features graphic
else
qemu_opts += -display none -nographic
endif
endif

run: build justrun
test: build-test justrun
debug: build debugrun

TERMINAL 	:= terminal
debugrun: $(QEMU_DISK)
	$(TERMINAL) -e "gdb -tui -x gdbinit"
	$(qemu) $(qemu_opts) -s -S

justrun: $(QEMU_DISK)
	$(qemu) $(qemu_opts)

build-test: build
	cp ../prebuilt/zircon/x64/core-tests.zbi $(ESP)/EFI/zCore/fuchsia.zbi
	echo 'cmdline=LOG=warn:userboot=test/core-standalone-test:userboot.shutdown:core-tests=$(test_filter)' >> $(ESP)/EFI/Boot/rboot.conf

build: $(kernel_img)

build-parallel-test: build $(QEMU_DISK)
	cp ../prebuilt/zircon/x64/core-tests.zbi $(ESP)/EFI/zCore/fuchsia.zbi
	echo 'cmdline=LOG=warn:userboot=test/core-standalone-test:userboot.shutdown:core-tests=$(test_filter)' >> $(ESP)/EFI/Boot/rboot.conf

$(kernel_img): kernel bootloader
	mkdir -p $(ESP)/EFI/zCore $(ESP)/EFI/Boot
	cp ../rboot/target/x86_64-unknown-uefi/release/rboot.efi $(ESP)/EFI/Boot/BootX64.efi
	cp rboot.conf $(ESP)/EFI/Boot/rboot.conf
ifeq ($(linux), 1)
	cp x86_64.img $(ESP)/EFI/zCore/fuchsia.zbi
else ifeq ($(user), 1)
	make -C ../zircon-user
	cp ../zircon-user/target/zcore.zbi $(ESP)/EFI/zCore/fuchsia.zbi
else
	cp ../prebuilt/zircon/x64/$(zbi_file).zbi $(ESP)/EFI/zCore/fuchsia.zbi
endif
	cp $(kernel) $(ESP)/EFI/zCore/zcore.elf

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

vbox: build
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

$(QEMU_DISK):
	qemu-img create -f qcow2 $@ 100M
