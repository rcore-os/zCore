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

.PNONY: loader zcore bridge simulate
