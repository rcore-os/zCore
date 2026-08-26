MODE ?= release
EFI := target/x86_64-unknown-uefi/$(MODE)/rboot.efi
OVMF := OVMF.fd
ESP := esp
QEMU_ARGS := -net none -smp cores=4 -nographic
#	-debugcon file:debug.log -global isa-debugcon.iobase=0x402

ifeq (${MODE}, release)
	BUILD_ARGS += --release
endif

.PHONY: build run header asm doc

build:
	cargo build $(BUILD_ARGS) --target x86_64-unknown-uefi -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem --ignore-rust-version

clippy:
	cargo clippy $(BUILD_ARGS) --target x86_64-unknown-uefi -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem

doc:
	cargo doc

uefi-run: build
	uefi-run \
		-b ${OVMF} \
		-q $(shell which qemu-system-x86_64) \
		$(EFI) \
		-- $(QEMU_ARGS)

run: build
	mkdir -p $(ESP)/EFI/Boot
	cp $(EFI) $(ESP)/EFI/Boot/BootX64.efi
	cp rboot.conf $(ESP)/EFI/Boot
	qemu-system-x86_64 \
		-bios ${OVMF} \
		-drive format=raw,file=fat:rw:${ESP} \
		$(QEMU_ARGS)

header:
	rust-objdump -h $(EFI) | less

asm:
	rust-objdump -d $(EFI) | less
