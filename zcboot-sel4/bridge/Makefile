all:
	mkdir build || true
	rm build/zc || true # force relinking Rust lib
	make -C zc_core
	cd build && cmake -G Ninja .. && ninja
