// Bring-up smoke test for the C-compile + FFI-link pipeline this crate will
// use to vendor NVIDIA's real open-gpu-kernel-modules source (MIT-licensed,
// src/nvidia/) unmodified into Eclipse. `vendor/smoketest.c` is NOT NVIDIA
// code -- it's a minimal hand-written translation unit that proves two
// things before any real vendoring work happens:
//   1. A C object file can be compiled with the exact freestanding flags
//      Eclipse's kernel target (zCore/x86_64.json: disable-redzone,
//      code-model=kernel, no MMX, panic=abort) requires, and linked into
//      the kernel binary.
//   2. C code can call back into a Rust-exported function -- the same
//      shape NVIDIA's RM uses to call into an os-interface.h implementation.
// Replace this file with real vendored NVIDIA sources once both are proven.
/// True when actually building for Eclipse's no_std kernel target
/// (CARGO_CFG_TARGET_OS reflects the JSON target spec's "os" field,
/// "none", regardless of how cc's own TARGET-string parsing handles a
/// non-triple custom target name).
fn building_for_kernel() -> bool {
    std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none")
}

/// Applies the freestanding kernel flags shared by every C translation
/// unit compiled into Eclipse's kernel binary.
fn apply_kernel_flags(build: &mut cc::Build) {
    if !building_for_kernel() {
        return;
    }
    // cc's own TARGET-string auto-detection rejects Cargo's custom JSON
    // target name ("x86_64" -- a single component, not a full triple):
    // "target `x86_64` only had a single component (at least two
    // required)". Override explicitly with the JSON's own "llvm-target"
    // value so cc's Unix-like defaults apply instead of erroring; this
    // does not change which compiler binary gets invoked (still the
    // host's cc/gcc), only cc's internal flag heuristics.
    build.target("x86_64-unknown-none");
    build
        .flag("-ffreestanding")
        .flag("-fno-builtin")
        .flag("-fno-stack-protector")
        .flag("-fno-asynchronous-unwind-tables")
        .flag("-mno-red-zone")
        // NOT -mcmodel=kernel: that GCC code model assumes the kernel
        // lives in the top 2GB of the address space (like Linux's own
        // 0xFFFFFFFF80000000 convention) and emits absolute 32-bit
        // sign-extended (R_X86_64_32S) relocations on that assumption.
        // zCore's actual KERNEL_BEGIN is 0xffffff0000000000 -- about 1.1
        // TB below that window -- so those relocations overflowed at
        // real link time ("relocation R_X86_64_32S out of range").
        // Explicit -fPIC keeps GCC on RIP-relative (R_X86_64_PC32)
        // addressing instead, which only depends on the DISTANCE between
        // code and data (both land in the same linked image, so it's
        // always in range), not the absolute base address -- confirmed
        // by comparing `readelf -r` output between the two flag sets on
        // a minimal reproduction before changing this for real.
        .flag("-fPIC")
        // Match zCore/x86_64.json's `"features": "-mmx,-sse,-sse2,+soft-float"`
        // exactly. The kernel's trap path (vendor/trapframe trap.S) saves only
        // general-purpose registers: there is no fxsave/fxrstor on a kernel
        // interrupt, so any XMM register live in interrupted kernel code is
        // corrupted on return. The Rust side is compiled SSE-free for that
        // reason; C code linked into the same kernel must be too, or it
        // reintroduces exactly the windows the Rust change closes.
        .flag("-mno-mmx")
        .flag("-mno-sse")
        .flag("-mno-sse2")
        .flag("-msoft-float")
        .flag("-nostdlib");
}

fn main() {
    let mut build = cc::Build::new();
    build.file("vendor/smoketest.c").flag_if_supported("-Wall");
    apply_kernel_flags(&mut build);
    build.compile("nvrm_smoketest");
    println!("cargo:rerun-if-changed=vendor/smoketest.c");

    build_first_real_nvidia_file();
}

/// Parses src/nvidia/srcs.mk -- NVIDIA's own real, authoritative list of
/// every .c file their build compiles into the Resource Manager core
/// (`SRCS += <path-relative-to-src/nvidia>`) -- rather than hand-picking
/// files and chasing undefined symbols one at a time. Confirmed by
/// actually compiling the full resulting set (1038 of 1054 entries) in an
/// isolated scratch build against the pinned submodule commit: it leaves
/// only the genuine os-interface.h/OBJOS boundary undefined, exactly the
/// surface os_interface.rs/os_services.rs/os_boundary.rs exist to fill.
///
/// Deliberately excludes `arch/nvalloc/unix/src/` -- NVIDIA's own Linux
/// platform-integration layer (the real `/dev/nvidia0` character device
/// driver, ioctls, registry access, etc.), which Eclipse replaces with
/// its own os_interface.rs implementation rather than vendoring.
fn parse_srcs_mk(nvidia_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let srcs_mk = nvidia_dir.join("srcs.mk");
    let text = std::fs::read_to_string(&srcs_mk)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", srcs_mk.display()));
    let mut files = Vec::new();
    for line in text.lines() {
        let Some(rel) = line.strip_prefix("SRCS += ") else {
            continue;
        };
        let rel = rel.trim();
        if rel.contains("arch/nvalloc/unix/src/") {
            continue;
        }
        files.push(nvidia_dir.join(rel));
    }
    files
}

/// Compiles the real NVIDIA RM source, which lives in the pinned
/// `open-gpu-kernel-modules` submodule.
///
/// For an x86_64 *kernel* build the submodule is REQUIRED: `zcore-drivers`
/// depends on this crate unconditionally, and the `eclipse_rm_*` glue in
/// `vendor/` needs NVIDIA's headers to compile, so without it the kernel does
/// not link. This used to print a warning and carry on "so the crate still
/// builds without it" — a promise it could not keep. What a fresh `git clone`
/// (no `--recursive`) actually got was ~50 `undefined symbol: eclipse_rm_*`
/// errors out of rust-lld, with nothing pointing at the submodule. Fail here
/// instead, where the message can say what to run.
///
/// Only for `target_os = "none"`, i.e. the bare-metal kernel targets that
/// actually link a binary. Host builds (`cargo build --all-features` in CI)
/// stop at rlibs, never resolve these symbols, and keep the old warning.
fn build_first_real_nvidia_file() {
    // The real NVIDIA RM core is x86_64 hardware: skip C compilation for
    // non-x86_64 targets to avoid cross-compilation issues (wrong-arch
    // object files) and missing-cross-compiler build failures.
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("x86_64") {
        println!(
            "cargo:warning=nvidia-rm-sys: skipping real NVIDIA C source for non-x86_64 target ({})",
            std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "unknown".to_string())
        );
        return;
    }
    let vendor = std::path::Path::new("vendor/open-gpu-kernel-modules");
    let nvidia = vendor.join("src/nvidia");
    if !nvidia.join("srcs.mk").exists() {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
            panic!(
                "nvidia-rm-sys: the open-gpu-kernel-modules submodule is not checked out \
                 ({} is missing), and the x86_64 kernel cannot link without it.\n\
                 \n\
                 Run:\n    git submodule update --init --recursive\n\
                 \n\
                 (or, for just this one: git submodule update --init \
                 nvidia-rm-sys/vendor/open-gpu-kernel-modules)",
                nvidia.join("srcs.mk").display()
            );
        }
        println!(
            "cargo:warning=nvidia-rm-sys: submodule not checked out ({} missing) -- skipping real NVIDIA source; fine for a host rlib build, but an x86_64 kernel link would fail",
            nvidia.join("srcs.mk").display()
        );
        return;
    }
    let source_files = parse_srcs_mk(&nvidia);
    // SRC_COMMON below is INFERRED as <submodule>/src/common (matches every
    // one of these paths existing under a "common" sibling of "nvidia" --
    // e.g. sdk/nvidia/inc, mbedtls/... -- but the exact `SRC_COMMON =`
    // assignment itself wasn't tracked down across kernel-open/Makefile,
    // src/nvidia/Makefile, and utils.mk). Verify/fix once this actually
    // runs against the checked-out submodule and reports real path errors.
    let common = vendor.join("src/common");

    // Transcribed from src/nvidia/Makefile's `CFLAGS += -I ...` lines, in
    // the same order, with $(SRC_COMMON) substituted, plus the extra
    // dirs below needed once the full nvswitch/nvlink kernel libraries
    // and the real libspdm third-party library (both part of NVIDIA's
    // own srcs.mk) are compiled in too.
    let include_dirs: [std::path::PathBuf; 35] = [
        nvidia.join("kernel/inc"),
        nvidia.join("interface"),
        common.join("sdk/nvidia/inc"),
        common.join("sdk/nvidia/inc/hw"),
        nvidia.join("arch/nvalloc/common/inc"),
        nvidia.join("arch/nvalloc/common/inc/gsp"),
        nvidia.join("arch/nvalloc/common/inc/deprecated"),
        nvidia.join("arch/nvalloc/unix/include"),
        nvidia.join("inc"),
        nvidia.join("inc/os"),
        common.join("shared/inc"),
        common.join("inc"),
        common.join("uproc/os/libos-v2.0.0/include"),
        common.join("uproc/os/common/include"),
        common.join("inc/swref"),
        common.join("inc/swref/published"),
        nvidia.join("generated"),
        common.join("nvswitch/kernel/inc"),
        common.join("nvswitch/interface"),
        common.join("nvswitch/common/inc"),
        common.join("inc/displayport"),
        common.join("nvlink/interface"),
        common.join("nvlink/inband/interface"),
        nvidia.join("inc/libraries"),
        nvidia.join("inc/kernel"),
        // NOT part of src/nvidia/Makefile's own -I list -- confirmed by a
        // real "No such file" failure that os-interface.h (and friends
        // like nvmisc.h, nvgputypes.h, rs_access.h, nv-caps.h) live under
        // kernel-open/, not src/nvidia/ or src/common/. The real build
        // must pass this in from the parent (kernel-open/Makefile) when
        // it recurses into src/nvidia's build; src/nvidia/Makefile alone
        // never mentions it.
        vendor.join("kernel-open/common/inc"),
        // NVSwitch/NVLink kernel-library internals (as opposed to just
        // their public interface headers above) -- needed once their
        // real .c files are compiled in (srcs.mk includes them
        // unconditionally; other kept files like kern_bus.c/gpu_mgr.c
        // reference their real API even on a system with neither piece
        // of hardware, confirmed empirically: excluding these files
        // orphaned MORE symbols than it removed).
        common.join("nvswitch/kernel"),
        common.join("nvlink/kernel/nvlink"),
        common.join("nvlink/kernel/nvlink/interface"),
        // libspdm (real, BSD-3-Clause-licensed DMTF SPDM reference
        // implementation NVIDIA vendors for Confidential-Computing
        // attestation) -- include paths transcribed from
        // src/nvidia/src/libraries/libspdm/nvidia/openspdm.mk. The 570.144
        // release vendors libspdm 3.1.1 (not 3.5.0), and 3.1.1 has no
        // os_stub/cryptlib_null directory, so that entry is dropped.
        nvidia.join("src/libraries/libspdm/3.1.1/include"),
        nvidia.join("src/libraries/libspdm/3.1.1/include/hal"),
        nvidia.join("src/libraries/libspdm/3.1.1/os_stub/include"),
        nvidia.join("src/libraries/libspdm/3.1.1/os_stub"),
        nvidia.join("src/libraries/libspdm/nvidia"),
        // GSP message-queue library (msgq.h) -- src/common/shared/msgq is
        // compiled into the RM core (msgq_utils.c etc.) and its public
        // header lives under inc/, which src/nvidia/Makefile pulls in when
        // it recurses; the standalone build needs it on the -I list.
        common.join("shared/msgq/inc"),
    ];

    let mut build = cc::Build::new();
    for f in &source_files {
        build.file(f);
    }
    // Our own shims (not NVIDIA source): vendor/glue.c provides nv_printf
    // (see that file for why); vendor/rm_boundary_stubs.c provides safe
    // "not implemented" bodies for real NVIDIA function signatures that
    // only matter for hardware/features Eclipse's target GPU doesn't have
    // (NVSwitch, NVLink, SPDM/Confidential Computing, the Linux ioctl
    // control-call surface) -- see that file's own header comment.
    build.file("vendor/glue.c");
    build.file("vendor/rm_boundary_stubs.c");
    // vendor/eclipse_rm_init.c: Eclipse's own equivalent of the Linux-
    // specific osRmInitRm/osInitNvMapping/RmInitAdapter orchestration in
    // arch/nvalloc/unix/src/osinit.c -- constructs the real OBJSYS
    // singleton and resource server, then attaches a GPU by real PCI
    // location and BAR info. See that file's header comment.
    build.file("vendor/eclipse_rm_init.c");
    // vendor/eclipse_rm_mem.c: Eclipse's real osAllocPagesInternal /
    // osFreePagesInternal / osMapSystemMemory / osUnmapSystemMemory,
    // replacing the no-op os_boundary.rs stubs -- backed by kernel-hal's
    // contiguous DMA frame allocator + physmap. See that file's header.
    build.file("vendor/eclipse_rm_mem.c");
    for dir in &include_dirs {
        build.include(dir);
    }
    // -include $(SRC_COMMON)/sdk/nvidia/inc/cpuopsys.h from the real
    // Makefile: force-included ahead of every translation unit, defines
    // the platform macros (NV_LINUX / NVCPU_* / etc.) most NVIDIA headers
    // key off of instead of detecting the compiler target themselves.
    build.flag(&format!(
        "-include{}",
        common.join("sdk/nvidia/inc/cpuopsys.h").display()
    ));

    // Transcribed from src/nvidia/Makefile's `CFLAGS += -D...` lines. First
    // real build against the checked-out submodule failed without these --
    // nvassert.h hard-#errors ("NV_PORT_HEADER must define
    // PORT_IS_CHECKED_BUILD") unless PORT_IS_CHECKED_BUILD is defined one
    // way or the other; NVIDIA's own real build uses the release (0) value.
    for def in [
        "PORT_IS_CHECKED_BUILD=0",
        "_LANGUAGE_C",
        "__NO_CTYPE",
        "NVRM",
        "LOCK_VAL_ENABLED=0",
        "PORT_ATOMIC_64_BIT_SUPPORTED=1",
        "PORT_IS_KERNEL_BUILD=1",
        "PORT_MODULE_atomic=1",
        "PORT_MODULE_core=1",
        "PORT_MODULE_cpu=1",
        "PORT_MODULE_crypto=1",
        "PORT_MODULE_debug=1",
        "PORT_MODULE_memory=1",
        "PORT_MODULE_safe=1",
        "PORT_MODULE_string=1",
        "PORT_MODULE_sync=1",
        "PORT_MODULE_thread=1",
        // 570.144 has NO nvport `time` module: its srcs.mk compiles no
        // time_generic.c and src/nvidia/inc/libraries/nvport/time.h does not
        // exist. nvport.h only pulls in "nvport/time.h" under
        // PORT_IS_MODULE_SUPPORTED(time), so this MUST be 0 for 570.144 --
        // forcing it to 1 (as 610.43.02's Makefile did) made nvport.h include
        // a nonexistent header ("fatal error: nvport/time.h"). RM 570 uses the
        // os-interface time hooks, not an nvport time module.
        "PORT_MODULE_time=0",
        "PORT_MODULE_util=1",
        "PORT_MODULE_example=0",
        "PORT_MODULE_mmio=0",
        "RS_STANDALONE=0",
        "RS_STANDALONE_TEST=0",
        "RS_COMPATABILITY_MODE=1",
        "RS_PROVIDES_API_STATE=0",
        "NV_CONTAINERS_NO_TEMPLATES",
        "INCLUDE_NVLINK_LIB",
        "INCLUDE_NVSWITCH_LIB",
        "NV_PRINTF_STRINGS_ALLOWED=1",
        "NV_ASSERT_FAILED_USES_STRINGS=1",
        "PORT_ASSERT_FAILED_USES_STRINGS=1",
        // NOT from the real Makefile -- deliberately overriding
        // nvassert.h's own default (on for NVRM+Unix builds) to avoid
        // needing NVIDIA's RCDB crash-journal subsystem (rcdbRmAssert)
        // just to log an assertion failure. nvassert.h explicitly
        // supports this override (`#if !defined(NV_JOURNAL_ASSERT_ENABLE)`).
        "NV_JOURNAL_ASSERT_ENABLE=0",
    ] {
        build.define(def, None);
    }
    // libspdm's own config-override mechanism: its headers do
    // `#include LIBSPDM_CONFIG` to pull in build-specific feature
    // toggles instead of hardcoding them, exactly like NVIDIA's real
    // openspdm.mk sets it.
    build.define("LIBSPDM_CONFIG", "<nvspdm_rmconfig.h>");

    apply_kernel_flags(&mut build);

    build.compile("nvrm_core");
    for f in &source_files {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    println!("cargo:rerun-if-changed=vendor/glue.c");
    println!("cargo:rerun-if-changed=vendor/rm_boundary_stubs.c");
    println!("cargo:rerun-if-changed=vendor/eclipse_rm_init.c");
    println!("cargo:rerun-if-changed=vendor/eclipse_rm_mem.c");
}
