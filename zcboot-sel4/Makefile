all: image

image: zcore bridge

zcore:
	cargo build --release

bridge: zcore
	mkdir bridge/build || true
	rm bridge/build/zc || true # force relinking Rust lib
	cd bridge/build && cmake -G Ninja .. && ninja

simulate: bridge
	cd bridge/build && ./simulate

.PNONY: zcore bridge simulate
