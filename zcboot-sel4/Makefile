all: image

image: zcore bridge

loader:
	make -C zc_loader

zcore: loader
	cargo xbuild --release --target x86_64-sel4.json

bridge: zcore
	make -C bridge

simulate: bridge
	cd bridge/build && ./simulate

simulate-gdb: bridge
	cd bridge/build && ./simulate --extra-qemu-args="-S -s"

.PNONY: loader zcore bridge simulate simulate-gdb
