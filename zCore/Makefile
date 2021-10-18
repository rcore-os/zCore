arch ?= x86_64
board ?= qemu
mode ?= debug
log ?= warn
zbi_file ?= bringup
graphic ?=
accel ?=
linux ?=
user ?=
hypervisor ?=
smp ?= 1
test_filter ?= *.*

build_args := -Z weak-dep-features -Z build-std=core,alloc --target $(arch).json
build_path := target/$(arch)/$(mode)
kernel := $(build_path)/zcore
kernel_img := $(build_path)/zcore.img
kernel_bin := $(build_path)/zcore.bin
ESP := $(build_path)/esp
OVMF := ../rboot/OVMF.fd
qemu := qemu-system-$(arch)
OBJDUMP := rust-objdump --print-imm-hex --x86-asm-syntax=intel
OBJCOPY := rust-objcopy --binary-architecture=$(arch)
VMDISK := $(build_path)/boot.vdi
QEMU_DISK := $(build_path)/disk.qcow2

export ARCH=$(arch)
export BOARD=$(board)
export USER_IMG=$(ARCH).img

ifeq ($(mode), release)
	build_args += --release
endif

ifeq ($(arch), riscv64)
ifeq ($(board), d1)
build_args += --features board_d1 --features link_user_img
else
build_args += --features board_qemu
endif
endif

ifeq ($(arch), x86_64)
build_args += --features ram_user_img
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
	-device ide-hd,drive=disk,bus=ahci.0 \
	-serial mon:stdio \
	-m 4G \
	-nic none \
	-device isa-debug-exit,iobase=0xf4,iosize=0x04
baremetal-test-qemu_opts += \
	-machine q35 \
	-cpu Haswell,+smap,-check,-fsgsbase \
	-drive if=pflash,format=raw,readonly,file=$(OVMF) \
	-drive format=raw,file=fat:rw:$(ESP) \
	-device ich9-ahci,id=ahci \
	-serial mon:stdio \
	-m 4G \
	-nic none \
	-device isa-debug-exit,iobase=0xf4,iosize=0x04
else ifeq ($(arch), riscv64)
qemu_opts += \
	-machine virt \
	-bios default \
	-serial mon:stdio \
	-no-reboot \
	-no-shutdown \
	-drive file=$(QEMU_DISK),format=qcow2,id=sfs \
	-device virtio-blk-device,drive=sfs \
	-kernel $(kernel_bin)

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

ifeq ($(arch), riscv64)
qemu_opts += \
	-device virtio-gpu-device \
	-device virtio-keyboard-device \
	-device virtio-mouse-device
else ifeq ($(arch), x86_64)
qemu_opts += -vga virtio # disable std VGA for zircon mode to avoid incorrect graphic rendering
endif

else

ifeq ($(MAKECMDGOALS), vbox)
build_args += --features graphic
endif

qemu_opts += -display none -nographic
baremetal-test-qemu_opts += -display none -nographic
endif

run: build justrun
test: build-test justrun
debug: build debugrun

TERMINAL 	:= gnome-terminal
debugrun: $(QEMU_DISK)
	$(TERMINAL) -e "gdb -tui -x gdbinit"
	$(qemu) $(qemu_opts) -s -S

justrun: $(QEMU_DISK)
	$(qemu) $(qemu_opts)

build-test: build
	cp ../prebuilt/zircon/x64/core-tests.zbi $(ESP)/EFI/zCore/fuchsia.zbi
	echo 'cmdline=LOG=$(log):userboot=test/core-standalone-test:userboot.shutdown:core-tests=$(test_filter)' >> $(ESP)/EFI/Boot/rboot.conf

build: $(kernel_img)

build-parallel-test: build $(QEMU_DISK)
	cp ../prebuilt/zircon/x64/core-tests.zbi $(ESP)/EFI/zCore/fuchsia.zbi
	echo 'cmdline=LOG=$(log):userboot=test/core-standalone-test:userboot.shutdown:core-tests=$(test_filter)' >> $(ESP)/EFI/Boot/rboot.conf

ifeq ($(arch), riscv64)
$(kernel_img): $(kernel_bin)

ifeq ($(board), d1)
run-thead: build
	@cp ../prebuilt/firmware/fw_jump-0x40020000.bin fw-zCore.bin
	@dd if=$(kernel_bin) of=fw-zCore.bin bs=1 seek=131072
	xfel ddr ddr3
	xfel write 0x40000000 fw-zCore.bin
	xfel exec 0x40000000
endif

else
$(kernel_img): kernel bootloader
	mkdir -p $(ESP)/EFI/zCore $(ESP)/EFI/Boot
	cp ../rboot/target/x86_64-unknown-uefi/release/rboot.efi $(ESP)/EFI/Boot/BootX64.efi
	cp rboot.conf $(ESP)/EFI/Boot/rboot.conf
ifeq ($(linux), 1) #root文件系统在里
	cp x86_64.img $(ESP)/EFI/zCore/fuchsia.zbi
else ifeq ($(user), 1)
	make -C ../zircon-user
	cp ../zircon-user/target/zcore.zbi $(ESP)/EFI/zCore/fuchsia.zbi
else
	cp ../prebuilt/zircon/x64/$(zbi_file).zbi $(ESP)/EFI/zCore/fuchsia.zbi
endif
	cp $(kernel) $(ESP)/EFI/zCore/zcore.elf

endif

kernel:
	echo Building zCore kenel
	cargo build $(build_args)

clippy:
	cargo clippy $(build_args)

bootloader:
	cd ../rboot && make build

$(kernel_bin): kernel
	$(OBJCOPY) $(kernel) --strip-all -O binary $@

clean:
	cargo clean -Z weak-dep-features

image:
	# for macOS only
	hdiutil create -fs fat32 -ov -volname EFI -format UDTO -srcfolder $(ESP) $(build_path)/zcore.cdr
	qemu-img convert -f raw $(build_path)/zcore.cdr -O qcow2 $(build_path)/zcore.qcow2

header:
	$(OBJDUMP) -x $(kernel) | less

disasm:
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
ifeq ($(arch), riscv64)
	@echo Generating riscv64 sfsimg
	@qemu-img convert -f raw riscv64.img -O qcow2 $@
	@qemu-img resize $@ +5M
else
	@qemu-img create -f qcow2 $@ 100M
endif

baremetal-qemu-disk:
	@qemu-img create -f qcow2 $(build_path)/disk.qcow2 100M

baremetal-test:
	cp rboot.conf $(ESP)/EFI/Boot/rboot.conf
	timeout --foreground 8s  $(qemu) $(baremetal-test-qemu_opts)

baremetal-test-rv64: build $(QEMU_DISK)
	timeout --foreground 8s $(qemu) $(qemu_opts) -append ROOTPROC=$(ROOTPROC)
