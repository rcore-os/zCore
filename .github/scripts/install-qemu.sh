#!/usr/bin/env bash

wget https://download.qemu.org/qemu-$1.tar.xz
tar -xf qemu-$1.tar.xz
cd qemu-$1
./configure --target-list=x86_64-softmmu,riscv64-softmmu
make -j
sudo make install
qemu-system-$2 --version