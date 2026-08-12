################ Arguments ################

ARCH ?= x86_64
PLATFORM ?= qemu
MODE ?= release
# warn (not error): the kernel routes its userspace-crash diagnostics through
# log::warn! — the "unhandled page fault ... -> SIGSEGV" line plus the [crash]
# dump of registers and the code bytes around the faulting PC (loader/src/
# linux.rs). LOG=error discarded ALL of that, so a segfaulting client (e.g.
# foot exiting rc=139) left nothing in dmesg to diagnose it with. Overridable:
# `make ... LOG=error` for a quiet build.
LOG ?= error
LINUX ?= 1
MOCK ?=
LIBOS ?=
TEST ?=
GRAPHIC ?=
DISK ?=
DISK_SIZE ?= 512M
HYPERVISOR ?=
V ?=

USER ?=
ZBI ?= bringup

SMP ?= 4
ACCEL ?=
IFACE ?= $(shell ip route show | grep default | grep -oE 'dev [^ ]+' | cut -d' ' -f2)
ifeq ($(IFACE),)
  IFACE := eno1
endif

NET ?=
OBJDUMP :=
OBJCOPY ?= rust-objcopy --binary-architecture=$(ARCH)

ifeq ($(ARCH), x86_64)
  OBJDUMP := rust-objdump --print-imm-hex --x86-asm-syntax=intel
else ifeq ($(ARCH), riscv64)
  OBJDUMP := riscv64-linux-musl-objdump
endif

# Optional desktop selector appended to the kernel cmdline as `desktop=<name>`.
# eclipse-init reads it from /proc/cmdline to pick the session (labwc vs xorg).
# Unset by default so real hardware falls through to /etc/eclipse/desktop; the
# top-level `make qemu` sets DESKTOP=xorg to boot the framebuffer X session.
desktop_arg := $(if $(DESKTOP),:desktop=$(DESKTOP))

# Optional boot-time file-access recorder appended as `BOOTTRACE=<comm>`.
# `make ... BOOTTRACE=labwc` makes the kernel record every file the `labwc`
# process opens (path + timestamp + result) and expose it at /proc/bootprofile
# as a startup timeline plus a deduplicated preload list. Unset by default
# (one relaxed atomic per openat when off). See linux-object/src/boot_trace.rs.
boottrace_arg := $(if $(BOOTTRACE),:BOOTTRACE=$(BOOTTRACE))

# GL renderer selector. DEFAULT: pixman (`GL ?= 0`) — the software 2D renderer
# that actually composites a frame. It appends `renderer=pixman` to the cmdline
# (eclipse-init then pins WLR_RENDERER=pixman + WLR_RENDERER_ALLOW_SOFTWARE) and
# keeps QEMU on plain `-vga virtio`.
#
# Why not GL by default: the virtio-gpu we expose to QEMU is 2D-ONLY — its DRM
# ioctl surface is a single GETPARAM stub, no virgl/3D (see
# drivers/src/virtio/gpu.rs). Mesa, seeing the `virtio_gpu` driver name, tries
# the virgl hardware path, finds no 3D ("virtio_gpu: driver missing"), and the
# software fallback then fails too — so on the GL path labwc never renders in
# QEMU. Hardware GL in QEMU would need the guest virgl protocol implemented
# (large) plus virglrenderer on the host.
#
# To opt INTO the GL path, build/run with
#     make ... GL=1
# `GL=1` appends `renderer=gl` (eclipse-init does NOT pin pixman, letting
# wlroots/Mesa pick the GPU renderer) and switches QEMU to a virgl-capable
# `virtio-vga-gl` display. Use it on real hardware once the nouveau GL stack
# (drivers/src/display/nvidia.rs) composites a frame on the RTX — NOT in QEMU,
# where it cannot render.
#
# NOTE for real hardware: the ESP's rboot.conf cmdline is written from CMDLINE,
# so a default build stamps `renderer=pixman` there. Build the installed image
# with `GL=1` to try nouveau GL, or edit `renderer=pixman` -> `renderer=gl` in
# the ESP's rboot.conf by hand.
GL ?= 0
gl_arg := $(if $(filter 0,$(GL)),:renderer=pixman,:renderer=gl)

ifeq ($(LINUX), 1)
  ifeq ($(ARCH), x86_64)
    CMDLINE ?= LOG=$(LOG):TERM=xterm-256color:console.shell=true:virtcon.disable=true:ROOT=/dev/vda:ROOTPROC=/bin/busybox?sh$(desktop_arg)$(boottrace_arg)$(gl_arg)
  else
    CMDLINE ?= LOG=$(LOG)$(desktop_arg)$(boottrace_arg)$(gl_arg)
  endif
else
  CMDLINE ?= LOG=$(LOG):TERM=xterm-256color:console.shell=true:virtcon.disable=true$(desktop_arg)$(boottrace_arg)$(gl_arg)
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

# `make qemu` used to hand QEMU the ESP *directory* via `-drive fat:rw:$(esp)`,
# whose built-in FAT emulation is FAT16 and caps at ~504 MiB. Once the image
# grew (the baked-in X.Org stack pushes the initramfs well past that), QEMU
# aborted: "Directory does not fit in FAT16". Build a real FAT32 image instead
# when the tools are available (mkfs.vfat + mtools), sized to the ESP contents
# — this is what a real hardware ESP is anyway. Fall back to the old directory
# method (fine for small images / hosts without the tools).
esp_img := $(build_path)/esp.img
have_vfat := $(shell command -v mkfs.vfat >/dev/null 2>&1 && command -v mcopy >/dev/null 2>&1 && command -v mmd >/dev/null 2>&1 && echo yes)
ifeq ($(have_vfat),yes)
  x86_esp_drive := -drive format=raw,file=$(esp_img)
else
  x86_esp_drive := -drive format=raw,file=fat:rw:$(esp)
endif

ifeq ($(shell uname), Darwin)
  sed := sed -i ""
else
  sed := sed -i
endif


################ Export environments ###################

export ARCH
export SMP
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
    else ifeq ($(PLATFORM), fu740)
	SMP := 5
	features += board-fu740
    else ifeq ($(PLATFORM), c910light)
	features += board-c910light link-user-img
    endif
  else ifeq ($(ARCH), aarch64)
  	ifeq ($(PLATFORM), raspi4b)
  	  features += link-user-img
  	endif
  endif
endif

ifeq ($(MOCK), 1)
  features += mock-disk
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
  build_args += --no-default-features --target $(ARCH).json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -Zjson-target-spec
endif

ifeq ($(MODE), release)
  build_args += --release
endif

ifeq ($(V), 1)
  build_args += --verbose
endif

################ QEMU options ################

qemu_opts := -smp $(SMP) -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0 -device usb-tablet,bus=xhci.0

ifeq ($(ARCH), x86_64)
  qemu_opts += \
		-machine q35 \
		-cpu Haswell,+smap,-check,-fsgsbase \
		-m 4G \
		-serial mon:stdio \
		-serial file:/tmp/serial.out \
		-drive format=raw,if=pflash,readonly=on,file=$(ovmf) \
		$(x86_esp_drive) \
		-nic none
else ifeq ($(ARCH), riscv64)
  qemu_opts += \
		-machine virt \
		-bios default \
		-m 1G \
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
	-netdev user,id=net1 \
	-device e1000e,netdev=net1 \
	-object filter-dump,id=f0,netdev=net1,file=/tmp/eclipse.pcap \
	-no-reboot

# Capture packets to dump.pcap only when explicitly requested (PCAP=on).
# The filter-dump hook adds a synchronous disk write per packet and can
# reduce bulk-download throughput by an order of magnitude.
ifeq ($(PCAP), on)
  qemu_opts += -object filter-dump,id=f1,netdev=net1,file=dump.pcap
endif

ifeq ($(DISK), on)
  ifeq ($(ARCH), x86_64)
    qemu_opts += -device virtio-blk-pci,drive=userdisk
  else ifeq ($(ARCH), riscv64)
    qemu_opts += -device virtio-blk-device,drive=userdisk
  endif
  qemu_opts += -drive format=qcow2,id=userdisk,if=none,file=$(qemu_disk)
endif

ifeq ($(GRAPHIC), on)
  ifeq ($(ARCH), x86_64)
    ifeq ($(GL), 1)
      # GL path (OPT-IN, GL=1): a virgl-capable virtio GPU + a GL-enabled host
      # display. Needs virglrenderer on the host; `-display gtk,gl=on` suits a
      # POP-OS/GTK desktop (use sdl,gl=on or egl-headless,gl=on if gtk is
      # unavailable). NOTE: even with this, our guest virtio-gpu is 2D-only (no
      # guest virgl impl), so Mesa still reports "virtio_gpu: driver missing" and
      # labwc does not render in QEMU — this switch exists for parity/experiments
      # and for the real-hardware nouveau path. Use pixman (default) for QEMU.
      qemu_opts += -device virtio-vga-gl -display gtk,gl=on
    else
      # DEFAULT (GL ?= 0): plain virtio VGA (no virgl), paired with renderer=pixman.
      qemu_opts += -vga virtio
    endif
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
ifeq ($(PLATFORM), fu740)
	gzip -9 -cvf $(build_path)/zcore.bin > ./zcore.bin.gz
	mkimage -f ../prebuilt/firmware/riscv/fu740_fdt.its ./zcore-fu740.itb
	@echo 'Build zcore-fu740.itb FIT-uImage done'
else ifeq ($(PLATFORM), c910light)
	mkimage -A riscv -O linux -C none -T kernel -a 0x200000 -e 0x200000 \
		-n "zCore for c910" \
		-d $(build_path)/zcore.bin uImageC910
endif
run: build justrun
debug: build debugrun
endif

.PHONY: justrun
justrun: $(qemu_disk)
ifeq ($(ARCH), x86_64)
	$(sed) 's#cmdline=.*#cmdline=$(CMDLINE)#' $(esp)/EFI/Boot/rboot.conf
  ifeq ($(have_vfat),yes)
	@esp_mb=$$(du -sm "$(esp)/EFI" | cut -f1); esp_mb=$$((esp_mb + 128)); \
	  echo "Building $(esp_img) ($$esp_mb MiB FAT32 — real ESP image; fat:rw dir emulation caps at FAT16 504 MiB)"; \
	  rm -f "$(esp_img)"; \
	  dd if=/dev/zero of="$(esp_img)" bs=1M count=$$esp_mb status=none; \
	  mkfs.vfat -F 32 "$(esp_img)" >/dev/null; \
	  mmd -i "$(esp_img)" ::/EFI ::/EFI/Boot ::/EFI/zCore; \
	  mcopy -i "$(esp_img)" -s "$(esp)/EFI" ::/
  endif
endif
ifeq ($(ARCH), aarch64)
	$(sed) 's#\"cmdline\":.*#\"cmdline\": \"$(CMDLINE)\",#' disk/EFI/Boot/Boot.json
endif
ifeq ($(PLATFORM), d1)
	$(OBJCOPY) ../prebuilt/firmware/riscv/d1_fw_payload.elf --strip-all -O binary ./zcore_d1.bin
	dd if=$(kernel_img) of=zcore_d1.bin bs=512 seek=2048
	xfel ddr ddr3
	xfel write 0x40000000 zcore_d1.bin
	xfel exec 0x40000000
else
	$(qemu) $(qemu_opts)
endif


ifeq ($(ARCH), x86_64)
  gdb := gdb
else ifeq ($(ARCH), riscv64)
  gdb := riscv64-unknown-elf-gdb
endif

.PHONY: debugrun
debugrun: $(qemu_disk)
	cp .gdbinit_$(ARCH) .gdbinit
ifeq ($(ARCH), x86_64)
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
disasm: build
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
	$(sed) 's#cmdline=.*#cmdline=$(CMDLINE)#' $(esp)/EFI/Boot/rboot.conf
# Strip debug info from the ESP copy of the kernel: the full ELF is ~137 MB of
# which only ~12 MB is loadable (PT_LOAD), and rboot reads the WHOLE file
# through the UEFI FAT driver at 50-200 MB/s — the fat ELF cost 0.7-2.7 s of
# every boot. --strip-debug (not --strip-all) keeps .symtab for in-kernel
# symbolization; the unstripped ELF stays at $(kernel_elf) for the
# llvm-addr2line crash-hunt workflow.
	@if command -v rust-objcopy >/dev/null 2>&1; then \
	  rust-objcopy --strip-debug $(kernel_elf) $(esp)/EFI/zCore/zcore.elf; \
	elif command -v objcopy >/dev/null 2>&1; then \
	  objcopy --strip-debug $(kernel_elf) $(esp)/EFI/zCore/zcore.elf; \
	else \
	  echo "objcopy not found; ESP kernel left unstripped (slower rboot load)"; \
	  cp $(kernel_elf) $(esp)/EFI/zCore/zcore.elf; \
	fi
	rm -f $(esp)/EFI/zCore/initramfs.img $(esp)/EFI/zCore/$(ARCH).img
	rm -f $(esp)/EFI/zCore/x86_64.img $(esp)/EFI/zCore/aarch64.img $(esp)/EFI/zCore/riscv64.img
# The initramfs only exists after `make rootfs`/`make image`. A build-only run
# (CI compiles the kernel without a rootfs) must still succeed: skip the copy
# with a notice instead of dying on `cp`.
	@if [ -f "$(user_img)" ]; then \
	  cp $(user_img) $(esp)/EFI/zCore/initramfs.img; \
	else \
	  echo "initramfs '$(user_img)' not present; skipping (build-only, no rootfs image)"; \
	fi
else ifeq ($(ARCH), riscv64)
	$(OBJCOPY) $(kernel_elf) --strip-all -O binary $@
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
	@bash $(abspath ../scripts/vbox-eclipse.sh)

$(qemu_disk):
ifeq ($(ARCH), riscv64)
# FIXME: no longer need to create QCOW2 when use initrd for RISC-V
	@echo Generating riscv64 sfsimg
	@qemu-img convert -f raw riscv64.img -O qcow2 $@
	@qemu-img resize $@ +5M
else
	@qemu-img create -f qcow2 $@ $(DISK_SIZE)
endif
