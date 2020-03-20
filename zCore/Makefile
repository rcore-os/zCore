arch ?= x86_64
mode ?= debug
LOG ?=
zbi_file ?= fuchsia

build_args := --target $(arch).json
build_path := target/$(arch)/$(mode)
kernel := $(build_path)/zcore
kernel_img := $(build_path)/zcore.img
ESP := $(build_path)/esp
OVMF := ../rboot/OVMF.fd
qemu := qemu-system-$(arch)
OBJDUMP := rust-objdump

ifeq ($(mode), release)
	build_args += --release
endif

qemu_opts := \
	-smp cores=1

ifeq ($(arch), x86_64)
qemu_opts += \
    -cpu qemu64,fsgsbase,rdrand \
	-drive if=pflash,format=raw,file=$(OVMF),readonly=on \
	-drive format=raw,file=fat:rw:$(ESP) \
	-serial mon:stdio \
	-m 4G \
	-device isa-debug-exit \
	-display none
endif

run: build justrun

debug: build debugrun

debugrun:
	$(qemu) $(qemu_opts) -s -S

justrun:
	$(qemu) $(qemu_opts)

build: $(kernel_img)

$(kernel_img): kernel bootloader
	mkdir -p $(ESP)/EFI/zCore $(ESP)/EFI/Boot
	cp ../rboot/target/x86_64-unknown-uefi/release/rboot.efi $(ESP)/EFI/Boot/BootX64.efi
	cp rboot.conf $(ESP)/EFI/Boot/rboot.conf
	cp ../prebuilt/zircon/$(zbi_file).zbi $(ESP)/EFI/zCore/fuchsia.zbi
	cp $(kernel) $(ESP)/EFI/zCore/zcore.elf

kernel:
	echo Building zCore kenel
	cargo xbuild $(build_args)

bootloader:
	cd ../rboot && make build

clean:
	cargo clean

header:
	$(OBJDUMP) -x $(kernel) | less

asm:
	$(OBJDUMP) -d $(kernel) | less
