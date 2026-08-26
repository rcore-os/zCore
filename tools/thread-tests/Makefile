# Freestanding multithreading regression tests for Eclipse OS.
#
#   make            builds both bare-metal and libos variants
#
# Run on bare metal: copy thr-metal/thr2-metal into the rootfs and boot with
#   ROOTPROC=/thr  (or /thr2)
# Run on libos: copy thr-libos/thr2-libos into rootfs/libos and run
#   cargo run -p zcore --release --features "linux libos" -- /thr

CFLAGS_COMMON := -nostdlib -fno-stack-protector -fno-builtin -O1

all: thr-metal thr-libos thr2-metal thr2-libos thr3-metal thr3-libos

thr-metal: thr.c
	gcc -static -no-pie $(CFLAGS_COMMON) -o $@ $<

thr2-metal: thr2.c
	gcc -static -no-pie $(CFLAGS_COMMON) -o $@ $<

thr3-metal: thr3.c
	gcc -static -no-pie $(CFLAGS_COMMON) -o $@ $<

# libos runs user code in-process; syscalls go through the patched
# `rcore_syscall_entry` trampoline, and the binary must be PIE because the
# process VMAR is placed at a non-zero base.
thr-libos: thr.c
	gcc -DVIA_CALL -static-pie -fPIC $(CFLAGS_COMMON) -o $@ $<

thr2-libos: thr2.c
	gcc -DVIA_CALL -static-pie -fPIC $(CFLAGS_COMMON) -o $@ $<

thr3-libos: thr3.c
	gcc -DVIA_CALL -static-pie -fPIC $(CFLAGS_COMMON) -o $@ $<

clean:
	rm -f thr-metal thr-libos thr2-metal thr2-libos thr3-metal thr3-libos

.PHONY: all clean
