MODE ?= debug

zbi_in := ../prebuilt/zircon/x64/bringup.zbi
zbi_out := target/zcore-user.zbi
build_dir := target/x86_64-fuchsia/$(MODE)
bootfs := $(build_dir)/bootfs
bins := $(patsubst src/bin/%.rs, $(bootfs)/bin/%, $(wildcard src/bin/*.rs))

ifeq ($(MODE), release)
  build_args += --release
endif

ifeq ($(shell uname), Darwin)
  zbi_cli := ../prebuilt/zircon/x64/zbi-macos
else
  zbi_cli := ../prebuilt/zircon/x64/zbi-linux
endif

.PHONY: zbi

all: zbi

zbi: $(zbi_out)

build:
	cargo build $(build_args)

$(bootfs)/bin/%: $(build_dir)/%
	mkdir -p $(bootfs)/bin
	cp $^ $@

$(zbi_out): build $(bins)
	$(zbi_cli) $(zbi_in) $(bootfs) -o $@
