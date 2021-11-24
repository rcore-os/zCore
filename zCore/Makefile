################ Arguments ################

ARCH ?= riscv64
PLATFORM ?= qemu
MODE ?= release
LOG ?= warn
LINUX ?= 1
LIBOS ?=
GRAPHIC ?=
HYPERVISOR ?=
V ?=

USER ?=
ZBI ?= bringup
CMDLINE ?=

SMP ?= 1
ACCEL ?=

OBJDUMP ?= rust-objdump --print-imm-hex --x86-asm-syntax=intel
OBJCOPY ?= rust-objcopy --binary-architecture=$(ARCH)

ifeq ($(LINUX), 1)
  user_img := $(ARCH).img
else ifeq ($(USER), 1)
  user_img := ../zircon-user/target/zcore-user.zbi
else
  user_img := ../prebuilt/zircon/x64/$(ZBI).zbi
endif

ifeq ($(PLATFORM), libos)
  LIBOS := 1
endif
ifeq ($(LIBOS), 1)
  build_path := ../target/$(MODE)
  PLATFORM := libos
  ifeq ($(LINUX), 1)
    ARGS ?= /bin/busybox
  else
    ARGS ?= $(user_img) $(CMDLINE)
  endif
else
  build_path := ../target/$(ARCH)/$(MODE)
endif

################ Internal variables ################

qemu := qemu-system-$(ARCH)
kernel_elf := $(build_path)/zcore
kernel_img := $(build_path)/zcore.bin
esp := $(build_path)/esp
ovmf := ../rboot/OVMF.fd
qemu_disk := $(build_path)/disk.qcow2

ifeq ($(shell uname), Darwin)
  sed := sed -i ""
else
  sed := sed -i
endif


################ Export environments ###################

export ARCH
export PLATFORM
export LOG
export USER_IMG=$(realpath $(user_img))

################ Cargo features ################

ifeq ($(LINUX), 1)
  features := linux
else
  features := zircon
endif

ifeq ($(LIBOS), 1)
  ifneq ($(ARCH), $(shell uname -m))
    $(error "ARCH" must be "$(shell uname -m)" for libos mode)
  endif
  features += libos
else
  ifeq ($(ARCH), riscv64)
    ifeq ($(PLATFORM), d1)
      features += board-d1 link-user-img
    else
      features += board-qemu
    endif
  endif
endif

ifeq ($(GRAPHIC), on)
  features += graphic
else ifeq ($(MAKECMDGOALS), vbox)
  features += graphic
endif

ifeq ($(HYPERVISOR), 1)
  features += hypervisor
  ACCEL := 1
endif

################ Cargo build args ################

build_args := --features "$(features)"

ifneq ($(LIBOS), 1) # no_std
  build_args += --no-default-features --target $(ARCH).json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
endif

ifeq ($(MODE), release)
  build_args += --release
endif

ifeq ($(V), 1)
  build_args += --verbose
endif

################ QEMU options ################

qemu_opts := -smp $(SMP)

ifeq ($(ARCH), x86_64)
  baremetal-test-qemu_opts := \
		-machine q35 \
		-cpu Haswell,+smap,-check,-fsgsbase \
		-m 4G \
		-serial mon:stdio \
		-drive format=raw,if=pflash,readonly=on,file=$(ovmf) \
		-drive format=raw,file=fat:rw:$(esp) \
		-device ich9-ahci,id=ahci \
		-device isa-debug-exit,iobase=0xf4,iosize=0x04 \
		-nic none
  qemu_opts += $(baremetal-test-qemu_opts) \
		-drive format=qcow2,id=userdisk,if=none,file=$(qemu_disk) \
		-device ide-hd,bus=ahci.0,drive=userdisk
else ifeq ($(ARCH), riscv64)
  qemu_opts += \
		-machine virt \
		-bios generic_fw_jump.bin \
		-m 512M \
		-no-reboot \
		-no-shutdown \
		-serial mon:stdio \
		-drive format=qcow2,id=userdisk,file=$(qemu_disk) \
		-device virtio-blk-device,drive=userdisk \
		-kernel $(kernel_img) \
		-initrd $(USER_IMG) \
		-append "LOG=$(LOG)"
endif

ifeq ($(GRAPHIC), on)
  ifeq ($(ARCH), x86_64)
    qemu_opts += -vga virtio # disable std VGA for zircon mode to avoid incorrect graphic rendering
  else ifeq ($(ARCH), riscv64)
    qemu_opts += \
		-device virtio-gpu-device \
		-device virtio-keyboard-device \
		-device virtio-mouse-device
  endif
else
  qemu_opts += -display none -nographic
  baremetal-test-qemu_opts += -display none -nographic
endif

ifeq ($(ACCEL), 1)
  ifeq ($(shell uname), Darwin)
    qemu_opts += -accel hvf
  else
    qemu_opts += -accel kvm -cpu host,migratable=no,+invtsc
  endif
endif

################ Make targets ################

.PHONY: all
all: build

.PHONY: build run test debug
ifeq ($(LIBOS), 1)
build: kernel
run:
	cargo run $(build_args) -- $(ARGS)
test:
	cargo test $(build_args)
debug: build
	gdb --args $(kernel_elf) $(ARGS)
else
build: $(kernel_img)
run: build justrun
debug: build debugrun
endif

.PHONY: justrun
justrun: $(qemu_disk)
	$(qemu) $(qemu_opts)

.PHONY: debugrun
debugrun: $(qemu_disk)
	$(qemu) $(qemu_opts) -s -S &
	@sleep 1
	gdb $(kernel_elf) -tui -x gdbinit

.PHONY: kernel
kernel:
	@echo Building zCore kenel
	cargo build $(build_args)

.PHONY: disasm
disasm:
	$(OBJDUMP) -d $(kernel_elf) | less

.PHONY: header
header:
	$(OBJDUMP) -x $(kernel_elf) | less

.PHONY: clippy
clippy:
	cargo clippy $(build_args)

.PHONY: clean
clean:
	cargo clean

.PHONY: bootloader
bootloader:
ifeq ($(ARCH), x86_64)
	@cd ../rboot && make build
endif

$(kernel_img): kernel bootloader
ifeq ($(ARCH), x86_64)
  ifeq ($(USER), 1)
	make -C ../zircon-user
  endif
	mkdir -p $(esp)/EFI/zCore $(esp)/EFI/Boot
	cp ../rboot/target/x86_64-unknown-uefi/release/rboot.efi $(esp)/EFI/Boot/BootX64.efi
	cp rboot.conf $(esp)/EFI/Boot/rboot.conf
	cp $(kernel_elf) $(esp)/EFI/zCore/zcore.elf
	cp $(user_img) $(esp)/EFI/zCore/
	$(sed) "s/fuchsia.zbi/$(notdir $(user_img))/" $(esp)/EFI/Boot/rboot.conf
  ifneq ($(CMDLINE),)
	$(sed) "s#cmdline=.*#cmdline=$(CMDLINE)#" $(esp)/EFI/Boot/rboot.conf
  else
	$(sed) "s/LOG=warn/LOG=$(LOG)/" $(esp)/EFI/Boot/rboot.conf
  endif
else ifeq ($(ARCH), riscv64)
	$(OBJCOPY) $(kernel_elf) --strip-all -O binary $@
endif

.PHONY: image
image:
# for macOS only
	hdiutil create -fs fat32 -ov -volname EFI -format UDTO -srcfolder $(esp) $(build_path)/zcore.cdr
	qemu-img convert -f raw $(build_path)/zcore.cdr -O qcow2 $(build_path)/zcore.qcow2

################ Tests ################

.PHONY: baremetal-qemu-disk
baremetal-qemu-disk:
	@qemu-img create -f qcow2 $(build_path)/disk.qcow2 100M

.PHONY: baremetal-test
baremetal-test:
	cp rboot.conf $(esp)/EFI/Boot/rboot.conf
	timeout --foreground 8s  $(qemu) $(baremetal-test-qemu_opts)

.PHONY: baremetal-test-rv64
baremetal-test-rv64: build $(qemu_disk)
	timeout --foreground 8s $(qemu) $(qemu_opts) -append ROOTPROC=$(ROOTPROC)

################ Deprecated ################

VMDISK := $(build_path)/boot.vdi

.PHONY: vbox
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

$(qemu_disk):
ifeq ($(ARCH), riscv64)
# FIXME: no longer need to create QCOW2 when use initrd for RISC-V
	@echo Generating riscv64 sfsimg
	@qemu-img convert -f raw riscv64.img -O qcow2 $@
	@qemu-img resize $@ +5M
else
	@qemu-img create -f qcow2 $@ 100M
endif
