#pragma once

/* default architecture for APK packages. */
#if defined(__x86_64__)
#define APK_DEFAULT_BASE_ARCH	"x86_64"
#elif defined(__i386__)
#define APK_DEFAULT_BASE_ARCH	"x86"
#elif defined(__powerpc__) && !defined(__powerpc64__)
#define APK_DEFAULT_BASE_ARCH	"ppc"
#elif defined(__powerpc64__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"ppc64"
#elif defined(__powerpc64__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"ppc64le"
#elif defined(__arm__) && defined(__ARM_PCS_VFP) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__ && __ARM_ARCH>=7
#define APK_DEFAULT_BASE_ARCH	"armv7"
#elif defined(__arm__) && defined(__ARM_PCS_VFP) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"armhf"
#elif defined(__arm__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"armel"
#elif defined(__arm__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"armeb"
#elif defined(__aarch64__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"aarch64"
#elif defined(__aarch64__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"aarch64_be"
#elif defined(__s390x__)
#define APK_DEFAULT_BASE_ARCH	"s390x"
#elif defined(__mips64) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"mips64"
#elif defined(__mips64) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"mips64el"
#elif defined(__mips__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"mips"
#elif defined(__mips__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"mipsel"
#elif defined(__riscv) && __riscv_xlen == 32
#define APK_DEFAULT_BASE_ARCH	"riscv32"
#elif defined(__riscv) && __riscv_xlen == 64
#define APK_DEFAULT_BASE_ARCH	"riscv64"
#elif defined(__loongarch__) && defined(__loongarch32) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"loongarch32"
#elif defined(__loongarch__) && defined(__loongarchx32) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"loongarchx32"
#elif defined(__loongarch__) && defined(__loongarch64) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"loongarch64"
#elif defined(__ARCHS__)
#define APK_DEFAULT_BASE_ARCH	"archs"
#elif defined(__ARC700__)
#define APK_DEFAULT_BASE_ARCH	"arc700"
#elif defined(__sh__) && defined(__SH2__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"sh2eb"
#elif defined(__sh__) && defined(__SH3__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"sh3"
#elif defined(__sh__) && defined(__SH4__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define APK_DEFAULT_BASE_ARCH	"sh4"
#elif defined(__wasi__)
#define APK_DEFAULT_BASE_ARCH	"wasi32"
#elif !defined(APK_CONFIG_ARCH)
#error APK_DEFAULT_BASE_ARCH not detected for this architecture
#endif

#if defined(APK_CONFIG_ARCH)
#define APK_DEFAULT_ARCH APK_CONFIG_ARCH
#elif defined(APK_CONFIG_ARCH_PREFIX)
#define APK_DEFAULT_ARCH APK_CONFIG_ARCH_PREFIX "-" APK_DEFAULT_BASE_ARCH
#else
#define APK_DEFAULT_ARCH APK_DEFAULT_BASE_ARCH
#endif
