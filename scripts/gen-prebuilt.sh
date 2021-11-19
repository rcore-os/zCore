# Generate prebuilt/zircon from modified fuchsia source

OUTDIR=zcore_prebuilt
ARCH=${1:-x64}
mkdir -p ${OUTDIR}

# set build target
./scripts/fx set bringup.${ARCH} --with-base //garnet/packages/tests:zircon --with //src/tests/microbenchmarks --with //src/virtualization/tests:hypervisor_tests_pkg

# apply zircon-libos.patch and build once
patch -p1 < zircon-libos.patch
./scripts/fx build default.zircon
patch -p1 -R < zircon-libos.patch
cp out/default.zircon/userboot-${ARCH}-clang/obj/kernel/lib/userabi/userboot/userboot.so ${OUTDIR}/userboot-libos.so
cp out/default.zircon/user.vdso-${ARCH}-clang.shlib/obj/system/ulib/zircon/libzircon.so.debug ${OUTDIR}/libzircon-libos.so

# apply zcore.patch and build again
patch -p1 < zcore.patch
./scripts/fx build
patch -p1 -R < zcore.patch
cp out/default.zircon/userboot-${ARCH}-clang/obj/kernel/lib/userabi/userboot/userboot.so ${OUTDIR}
cp out/default.zircon/user.vdso-${ARCH}-clang.shlib/obj/system/ulib/zircon/libzircon.so.debug ${OUTDIR}/libzircon.so
cp out/default/bringup.zbi ${OUTDIR}
cp out/default/obj/zircon/system/utest/core/core-tests.zbi ${OUTDIR}

# remove kernel and cmdline from zbi
cd ${OUTDIR}
../out/default.zircon/tools/zbi -x bringup.zbi -D bootfs
../out/default.zircon/tools/zbi bootfs -o bringup.zbi
rm -r bootfs
cd ..

# finished
echo 'generate prebuilt at' ${OUTDIR}
