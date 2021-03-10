#!/bin/bash

# rustc +nightly -Z unstable-options --target=riscv64imac-unknown-none-elf --print target-spec-json

# riscv64根文件系统 
# make rv64-image

#make run linux=1 accel=1 $@

#cargo build -Z build-std=core,alloc --target riscv64.json --features linux
make run linux=1 arch=riscv64 $@

