all:
	mkdir build || true
	rm build/archive.o || true # force rebuild
	cd build && cmake -G Ninja .. && ninja
