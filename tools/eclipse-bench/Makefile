# eclipse-bench — CPU / memory / disk / process benchmark for Eclipse OS.
#
# Statically linked against musl so the single binary drops straight into the
# rootfs and runs from the shell. Override CC if your musl cross-compiler is
# named or located differently:
#
#   make                              # uses x86_64-linux-musl-gcc
#   make CC=musl-gcc
#   make CC=/path/to/x86_64-linux-musl-gcc
#
# Then copy `eclipse-bench` into the rootfs (e.g. /root or /bin) and run it on
# Eclipse. See README.md.

CC     ?= x86_64-linux-musl-gcc
# -pthread: the SMP-scaling and thread-context-switch sections use pthreads.
# Static musl links them in without a separate runtime.
CFLAGS ?= -O2 -static -pthread -Wall -Wextra

eclipse-bench: eclipse-bench.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f eclipse-bench

.PHONY: clean
