#!/bin/sh
# Build sysbench 1.0.20 (dynamic, musl) and verify it runs on Eclipse OS.
#
# This is the binary used to confirm the threading fixes: real sysbench with
# real musl pthreads + bundled LuaJIT, i.e. exactly the program that hung at
# "Initializing worker threads...". With the fixed kernel it reaches
# "Threads started!" and finishes the CPU benchmark.
#
# Requires: musl-gcc (apt: musl-tools musl-dev), autoconf/automake/libtool,
# network access to github.com (for the sysbench tarball).
set -e

SB_VER=1.0.20
WORK=${WORK:-/tmp/sysbench-build}
mkdir -p "$WORK"
cd "$WORK"

[ -f sysbench.tar.gz ] || curl -sL -o sysbench.tar.gz \
    "https://github.com/akopytov/sysbench/archive/refs/tags/${SB_VER}.tar.gz"
[ -d "sysbench-${SB_VER}" ] || tar xzf sysbench.tar.gz
cd "sysbench-${SB_VER}"

./autogen.sh
CC=musl-gcc CFLAGS="-O2" ./configure --without-mysql   # bundled LuaJIT + CK

# LuaJIT/concurrency_kit only fail building their .so under static musl; their
# .a archives build fine. Force static and stage the headers/libs by hand.
sed -i 's/^BUILDMODE=.*/BUILDMODE=static/' third_party/luajit/luajit/src/Makefile || true
make -j"$(nproc)" LDFLAGS="-static" || true
mkdir -p third_party/concurrency_kit/lib third_party/concurrency_kit/include
cp third_party/concurrency_kit/tmp/ck/src/libck.a third_party/concurrency_kit/lib/ 2>/dev/null || true
cp -r third_party/concurrency_kit/tmp/ck/include/* third_party/concurrency_kit/include/ 2>/dev/null || true

# musl lacks the glibc *64 LFS aliases and _dl_find_object that LuaJIT
# references; provide a tiny shim object.
cat > shim64.c <<'EOF'
#include <stdio.h>
#include <sys/mman.h>
#include <fcntl.h>
#include <stdlib.h>
#include <stdarg.h>
FILE *fopen64(const char *p,const char *m){return fopen(p,m);}
int open64(const char *p,int fl,...){va_list a;va_start(a,fl);int mode=va_arg(a,int);va_end(a);return open(p,fl,mode);}
void *mmap64(void *a,size_t l,int pr,int fl,int fd,long o){return mmap(a,l,pr,fl,fd,(off_t)o);}
int fseeko64(FILE *f,long o,int w){return fseeko(f,(off_t)o,w);}
long ftello64(FILE *f){return (long)ftello(f);}
FILE *tmpfile64(void){return tmpfile();}
int mkstemp64(char *t){return mkstemp(t);}
int _dl_find_object(void *a,void *b){(void)a;(void)b;return -1;}
EOF
musl-gcc -O2 -fno-builtin -c shim64.c -o shim64.o
sed -i 's#^sysbench_LDADD = #sysbench_LDADD = '"$PWD"'/shim64.o #' src/Makefile
( cd src && make LDFLAGS="-static" )

echo "Built: $PWD/src/sysbench"
echo "Copy it (and /lib/ld-musl-x86_64.so.1) into the Eclipse rootfs, then:"
echo "    sysbench cpu --time=10 --threads=4 run"
