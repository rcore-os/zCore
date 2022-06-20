################ Arguments ################

ARCH ?= x86_64
PLATFORM ?= qemu
MODE ?= release
LOG ?= warn
LINUX ?=
LIBOS ?=
TEST ?=
GRAPHIC ?=
DISK ?=
HYPERVISOR ?=
V ?=

USER ?=
ZBI ?= bringup

SMP ?= 1
ACCEL ?=

NET ?=
OBJDUMP :=
OBJCOPY ?= rust-objcopy --binary-architecture=$(ARCH)

ifeq ($(ARCH), x86_64)
  OBJDUMP := rust-objdump --print-imm-hex --x86-asm-syntax=intel
else ifeq ($(ARCH), riscv64)
  OBJDUMP := riscv64-linux-musl-objdump
endif

ifeq ($(LINUX), 1)
  CMDLINE ?= LOG=$(LOG)
else
  CMDLINE ?= LOG=$(LOG):TERM=xterm-256color:console.shell=true:virtcon.disable=true
endif

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
  else ifeq ($(ARCH), aarch64)
  	ifeq ($(PLATFORM), raspi4b)
  	  features += link-user-img
  	endif
  endif
endif

ifeq ($(TEST), 1)
  features += baremetal-test
  NET := loopback
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

ifeq ($(NET), loopback)
  features += loopback
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
  qemu_opts += \
		-machine q35 \
		-cpu Haswell,+smap,-check,-fsgsbase \
		-m 1G \
		-serial mon:stdio \
		-serial file:/tmp/serial.out \
		-drive format=raw,if=pflash,readonly=on,file=$(ovmf) \
		-drive format=raw,file=fat:rw:$(esp) \
		-nic none
else ifeq ($(ARCH), riscv64)
  qemu_opts += \
		-machine virt \
		-bios default \
		-m 512M \
		-no-reboot \
		-serial mon:stdio \
		-serial file:/tmp/serial.out \
		-kernel $(kernel_img) \
		-initrd $(USER_IMG) \
		-append "$(CMDLINE)"
else ifeq ($(ARCH), aarch64)
	qemu_opts += \
		-machine virt \
		-cpu cortex-a72 \
		-m 1G \
		-serial mon:stdio \
		-serial file:/tmp/serial.out \
		-bios ../ignored/target/aarch64/firmware/QEMU_EFI.fd \
		-hda fat:rw:disk \
		-drive file=aarch64.img,if=none,format=raw,id=x0 \
		-device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
endif

qemu_opts += \
	-netdev user,id=net1,hostfwd=tcp::8000-:80,hostfwd=tcp::2222-:2222,hostfwd=udp::6969-:6969 \
	-device e1000e,netdev=net1

ifeq ($(DISK), on)
  ifeq ($(ARCH), x86_64)
    qemu_opts += -device ide-hd,bus=ahci.0,drive=userdisk
  else ifeq ($(ARCH), riscv64)
    qemu_opts += -device virtio-blk-device,drive=userdisk
  endif
  qemu_opts += -drive format=qcow2,id=userdisk,if=none,file=$(qemu_disk)
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
  qemu_opts += -display none
endif

ifeq ($(ARCH), x86_64)
  ifeq ($(PLATFORM), qemu)
    ifeq ($(ACCEL), 1)
      ifeq ($(shell uname), Darwin)
        qemu_opts += -accel hvf
      else
        qemu_opts += -accel kvm -cpu host,migratable=no,+invtsc
      endif
	endif
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
ifeq ($(ARCH), x86_64)
	$(sed) 's#initramfs=.*#initramfs=\\EFI\\zCore\\$(notdir $(user_img))#' $(esp)/EFI/Boot/rboot.conf
	$(sed) 's#cmdline=.*#cmdline=$(CMDLINE)#' $(esp)/EFI/Boot/rboot.conf
endif
ifeq ($(ARCH), aarch64)
	$(sed) 's#\"cmdline\":.*#\"cmdline\": \"$(CMDLINE)\",#' disk/EFI/Boot/Boot.json
endif
	$(qemu) $(qemu_opts)

ifeq ($(ARCH), x86_64)
  gdb := gdb
else ifeq ($(ARCH), riscv64)
  gdb := riscv64-unknown-elf-gdb
endif

.PHONY: debugrun
debugrun: $(qemu_disk)
	cp .gdbinit_$(ARCH) .gdbinit
ifeq ($(ARCH), x86_64)
	$(sed) 's#initramfs=.*#initramfs=\\EFI\\zCore\\$(notdir $(user_img))#' $(esp)/EFI/Boot/rboot.conf
	$(sed) 's#cmdline=.*#cmdline=$(CMDLINE)#' $(esp)/EFI/Boot/rboot.conf
endif
ifeq ($(ARCH), aarch64)
	$(sed) 's#\"cmdline\":.*#\"cmdline\": \"$(CMDLINE)\",#' disk/EFI/Boot/Boot.json
endif
	$(qemu) $(qemu_opts) -S -gdb tcp::15234 &
	@sleep 1
	$(gdb)

.PHONY: kernel
kernel:
	@echo Building zCore kernel
	SMP=$(SMP) cargo build $(build_args)
ifeq ($(ARCH), aarch64)
	@mkdir -p disk/EFI/Boot
	@cp ../target/aarch64/$(MODE)/zcore disk/os
endif

.PHONY: disasm
disasm:
	$(OBJDUMP) -d $(kernel_elf) > kernel.asm

.PHONY: header
header:
	$(OBJDUMP) -x $(kernel_elf) | less

.PHONY: clippy
clippy:
	SMP=$(SMP) cargo clippy $(build_args)

.PHONY: clean
clean:
	cargo clean
	@rm -rf disk

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
else ifeq ($(ARCH), riscv64)
	$(OBJCOPY) $(kernel_elf) --strip-all -O binary $@
endif

ifeq ($(ARCH), riscv64)
ifeq ($(PLATFORM), d1)
.PHONY: run_d1
run_d1: build
	$(OBJCOPY) ../prebuilt/firmware/d1/fw_payload.elf --strip-all -O binary ./zcore_d1.bin
	dd if=$(kernel_img) of=zcore_d1.bin bs=512 seek=2048
	xfel ddr d1
	xfel write 0x40000000 zcore_d1.bin
	xfel exec 0x40000000
endif
endif

.PHONY: image
image:
# for macOS only
	hdiutil create -fs fat32 -ov -volname EFI -format UDTO -srcfolder $(esp) $(build_path)/zcore.cdr
	qemu-img convert -f raw $(build_path)/zcore.cdr -O qcow2 $(build_path)/zcore.qcow2

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
