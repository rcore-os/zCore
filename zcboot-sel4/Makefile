all: image

image: zcore bridge

zcore:
	cargo build --release

bridge: zcore
	make -C bridge

simulate: bridge
	cd bridge/build && ./simulate

.PNONY: zcore bridge simulate
