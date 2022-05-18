#!/usr/bin/env bash

wget https://download.qemu.org/qemu-$1.tar.xz
tar -xJf qemu-$1.tar.xz
cd qemu-$1
./configure --target-list=x86_64-softmmu,riscv64-softmmu
make -j$nproc > /dev/null 2>&1