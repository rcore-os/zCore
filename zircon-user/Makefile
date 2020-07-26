mode ?= debug

ZBI_IN := ../prebuilt/zircon/x64/bringup.zbi
ZBI_OUT := target/zcore.zbi
BUILD_DIR := target/x86_64-fuchsia/$(mode)
BOOTFS := $(BUILD_DIR)/bootfs
BINS := $(patsubst src/bin/%.rs, $(BOOTFS)/bin/%, $(wildcard src/bin/*.rs))

ifeq ($(mode), release)
	BUILD_ARGS += --release
endif

ifeq ($(shell uname), Darwin)
	ZBI_CLI := ../prebuilt/zircon/x64/zbi-macos
else
	ZBI_CLI := ../prebuilt/zircon/x64/zbi-linux
endif

.PHONY: zbi

all: zbi

zbi: $(ZBI_OUT)

build:
	cargo build $(BUILD_ARGS)

$(BOOTFS)/bin/%: $(BUILD_DIR)/%
	mkdir -p $(BOOTFS)/bin
	cp $^ $@

$(ZBI_OUT): build $(BINS)
	$(ZBI_CLI) $(ZBI_IN) $(BOOTFS) -o $@
