# Generate prebuilt/zircon from modified fuchsia source

OUTDIR=zcore_prebuilt
mkdir -p ${OUTDIR}

# set build target
./scripts/fx set bringup.x64 --with-base //garnet/packages/tests:zircon --with //src/tests/microbenchmarks

# apply zircon-libos.patch and build once
patch -p1 < zircon-libos.patch
./scripts/fx build default.zircon
patch -p1 -R < zircon-libos.patch
cp out/default.zircon/userboot-x64-clang/obj/kernel/lib/userabi/userboot/userboot.so ${OUTDIR}/userboot-libos.os
cp out/default.zircon/user.vdso-x64-clang.shlib/obj/system/ulib/zircon/libzircon.so.debug ${OUTDIR}/libzircon-libos.so

# apply zcore.patch and build again
patch -p1 < zcore.patch
./scripts/fx build
patch -p1 -R < zcore.patch
cp out/default.zircon/userboot-x64-clang/obj/kernel/lib/userabi/userboot/userboot.so ${OUTDIR}
cp out/default.zircon/user.vdso-x64-clang.shlib/obj/system/ulib/zircon/libzircon.so.debug ${OUTDIR}/libzircon.so
cp out/default.zircon/userboot-x64-clang/obj/system/ulib/hermetic-decompressor/decompress-zstd.so ${OUTDIR}
cp out/default/bringup.zbi ${OUTDIR}
# remove kernel and cmdline from zbi
cd ${OUTDIR}
../out/default.zircon/tools/zbi -x bringup.zbi -D bootfs
../out/default.zircon/tools/zbi bootfs -o bringup.zbi
rm -r bootfs
cd ..

# finished
echo 'generate prebuilt at' ${OUTDIR}
