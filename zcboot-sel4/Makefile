all: image

image: zcore bridge

loader:
	make -C zc_loader

zcore: loader
	cargo xbuild --release --target x86_64-sel4.json

bridge: zcore
	make -C bridge

simulate: bridge
	cd bridge/build && ./simulate --cpu Skylake-Client-IBRS,vmx=off,pcid=on --extra-qemu-args="-m 512M"

simulate-kvm: bridge
	cd bridge/build && sudo ./simulate --cpu Skylake-Client-IBRS,vmx=off,pcid=on --extra-qemu-args="-m 512M --enable-kvm"

simulate-gdb: bridge
	cd bridge/build && ./simulate --extra-qemu-args="-S -s"

.PNONY: loader zcore bridge simulate simulate-gdb
