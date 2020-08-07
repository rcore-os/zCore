BUILD_DIR := ../bridge/build
SEL4_DIR := ../../../seL4

all:
	clang \
		-c \
		-nostdlib -static -fPIC -O2 \
		-I$(BUILD_DIR)/kernel/gen_config/ \
		-I$(BUILD_DIR)/libsel4/gen_config/ \
		-I$(BUILD_DIR)/libsel4/autoconf/ \
		-I$(BUILD_DIR)/libsel4/sel4_arch_include/x86_64/ \
		-I$(BUILD_DIR)/libsel4/include/ \
		-I$(BUILD_DIR)/libsel4/arch_include/x86/ \
		-I$(SEL4_DIR)/libsel4/sel4_arch_include/x86_64/ \
		-I$(SEL4_DIR)/libsel4/arch_include/x86/ \
		-I$(SEL4_DIR)/libsel4/include/ \
		-I$(SEL4_DIR)/libsel4/mode_include/64/ \
		-I$(SEL4_DIR)/libsel4/sel4_plat_include/pc99/ \
		loader.c
	rust-ar \
		rc libzc_loader.a \
		loader.o

.PHONY: all