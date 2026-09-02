ifeq ($(ARCH), x86_64)
TARGET := x86_64-unknown-linux-gnu
else ifeq ($(ARCH), aarch64)
TARGET := aarch64-unknown-linux-gnu
else ifeq ($(ARCH), mipsel)
TARGET := mipsel-unknown-linux-gnu
else ifeq ($(ARCH), riscv32)
TARGET := riscv32imac-unknown-none-elf
else ifeq ($(ARCH), riscv64)
TARGET := riscv64imac-unknown-none-elf
endif

.PHONY: env build clippy doc

all: build

env:
	rustup target add $(TARGET)

build:
	cargo build --target $(TARGET)

clippy:
	cargo clippy --target $(TARGET)

doc:
	cargo doc --target $(TARGET) --no-deps
