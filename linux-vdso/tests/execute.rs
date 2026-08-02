//! Runs the linked vDSO image.
//!
//! The build script proves the image's metadata is what a C library expects.
//! This proves the code inside it is right, which no amount of ELF inspection
//! can: it maps the image the way the kernel will, fills `_vdso_data` the way
//! the kernel will, and calls `__vdso_clock_gettime` through a function pointer
//! the way musl will — on the host, in milliseconds, instead of inside a guest
//! forty minutes later.
//!
//! Two properties matter most and are the hardest to eyeball. The 64x64->128
//! fixed-point multiply is easy to get subtly wrong in a way that only shows up
//! at large TSC values (the whole reason it is not plain 64-bit arithmetic), so
//! it is checked against an independent computation at a realistic uptime. And
//! every path this image cannot serve must return something musl treats as
//! "fall back to the syscall" — never `-EINVAL`, which musl passes straight to
//! the caller as a hard error.
//!
//! Only meaningful where the host can execute the image, i.e. x86_64 Linux.

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::sync::atomic::{compiler_fence, Ordering};

const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;
const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
const ENOSYS: i32 = -38;

/// The `struct timespec` the image writes through.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

impl Timespec {
    fn as_ns(&self) -> u128 {
        self.tv_sec as u128 * 1_000_000_000 + self.tv_nsec as u128
    }
}

type ClockGettime = unsafe extern "C" fn(i32, *mut Timespec) -> i32;
type Gettimeofday = unsafe extern "C" fn(*mut Timeval, *mut Timezone) -> i32;
type Time = unsafe extern "C" fn(*mut i64) -> i64;

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct Timeval {
    tv_sec: i64,
    tv_usec: i64,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct Timezone {
    tz_minuteswest: i32,
    tz_dsttime: i32,
}

fn rdtsc() -> u64 {
    // `compiler_fence` only orders the compiler, which is all this needs: the
    // test compares against a window, not an exact instant, so an out-of-order
    // `rdtsc` on the CPU cannot make it flaky.
    compiler_fence(Ordering::SeqCst);
    let (lo, hi): (u32, u32);
    unsafe {
        std::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    compiler_fence(Ordering::SeqCst);
    ((hi as u64) << 32) | lo as u64
}

/// The image mapped at a real address, with executable pages.
struct Mapping {
    base: *mut u8,
    len: usize,
}

impl Mapping {
    /// Maps the image and publishes `data` into it, exactly as the kernel does:
    /// copy the image, write the clock parameters into the `_vdso_data` page,
    /// then make the whole thing read-execute.
    fn new(data: linux_vdso::VdsoData) -> Self {
        let len = (linux_vdso::IMAGE_LEN + 0xfff) & !0xfff;
        let base = unsafe { mmap_rw(len) };
        assert!(!base.is_null(), "mmap fallo");

        unsafe {
            std::ptr::copy_nonoverlapping(linux_vdso::IMAGE.as_ptr(), base, linux_vdso::IMAGE_LEN);
            let slot = base.add(linux_vdso::DATA_OFFSET) as *mut linux_vdso::VdsoData;
            slot.write(data);
            // Read-execute, not read-write-execute: the guest never gets write
            // access to this either, so the test should not either.
            assert!(mprotect_rx(base, len), "mprotect fallo");
        }
        Mapping { base, len }
    }

    fn sym<T: Copy>(&self, name: &str) -> T {
        assert_eq!(
            std::mem::size_of::<T>(),
            std::mem::size_of::<usize>(),
            "se esperaba un puntero a funcion"
        );
        let off = linux_vdso::vdsosym(linux_vdso::IMAGE, name.as_bytes())
            .unwrap_or_else(|| panic!("{} no resuelve en la imagen", name));
        unsafe {
            let addr = self.base.add(off as usize);
            *(&addr as *const *mut u8 as *const T)
        }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe { munmap(self.base, self.len) };
    }
}

unsafe fn mmap_rw(len: usize) -> *mut u8 {
    let ret: isize;
    std::arch::asm!(
        "syscall",
        inlateout("rax") 9isize => ret,      // SYS_mmap
        in("rdi") 0usize,                    // addr = NULL
        in("rsi") len,
        in("rdx") 3usize,                    // PROT_READ | PROT_WRITE
        in("r10") 0x22usize,                 // MAP_PRIVATE | MAP_ANONYMOUS
        in("r8") -1isize,                    // fd
        in("r9") 0usize,                     // offset
        lateout("rcx") _, lateout("r11") _,
        options(nostack)
    );
    if ret < 0 {
        std::ptr::null_mut()
    } else {
        ret as *mut u8
    }
}

unsafe fn mprotect_rx(addr: *mut u8, len: usize) -> bool {
    let ret: isize;
    std::arch::asm!(
        "syscall",
        inlateout("rax") 10isize => ret,     // SYS_mprotect
        in("rdi") addr,
        in("rsi") len,
        in("rdx") 5usize,                    // PROT_READ | PROT_EXEC
        lateout("rcx") _, lateout("r11") _,
        options(nostack)
    );
    ret == 0
}

unsafe fn munmap(addr: *mut u8, len: usize) {
    std::arch::asm!(
        "syscall",
        inlateout("rax") 11isize => _,       // SYS_munmap
        in("rdi") addr,
        in("rsi") len,
        lateout("rcx") _, lateout("r11") _,
        options(nostack)
    );
}

/// A plausible calibration: 1 GHz, so `tsc_mult` is exactly 1 ns per tick and
/// the expected answers can be written down by hand.
const ONE_GHZ_MULT: u64 = 1u64 << 32;

fn enabled(wall_off_ns: u64) -> linux_vdso::VdsoData {
    linux_vdso::VdsoData {
        enabled: 1,
        _pad: 0,
        tsc_mult: ONE_GHZ_MULT,
        wall_off_ns,
    }
}

#[test]
fn image_was_built() {
    assert!(
        linux_vdso::AVAILABLE,
        "sin imagen: este test necesita un cc capaz de generar x86_64"
    );
    assert_eq!(linux_vdso::IMAGE.len(), linux_vdso::IMAGE_LEN);
    assert_eq!(linux_vdso::DATA_OFFSET % 4096, 0);
}

#[test]
fn monotonic_tracks_the_tsc() {
    let m = Mapping::new(enabled(0));
    let clock_gettime: ClockGettime = m.sym("__vdso_clock_gettime");

    // Bracket the call between two TSC reads. At 1 ns/tick the answer must land
    // inside the bracket, which pins down the whole conversion — a wrong shift
    // or a truncated multiply lands orders of magnitude away.
    let before = rdtsc();
    let mut ts = Timespec::default();
    let r = unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
    let after = rdtsc();

    assert_eq!(r, 0, "CLOCK_MONOTONIC deberia responderse en espacio de usuario");
    let ns = ts.as_ns();
    assert!(
        ns >= before as u128 && ns <= after as u128,
        "monotonic {} fuera del intervalo [{}, {}]",
        ns,
        before,
        after
    );
    assert!(ts.tv_nsec >= 0 && ts.tv_nsec < 1_000_000_000, "tv_nsec fuera de rango: {:?}", ts);
}

#[test]
fn monotonic_never_goes_backwards() {
    let m = Mapping::new(enabled(0));
    let clock_gettime: ClockGettime = m.sym("__vdso_clock_gettime");

    let mut prev = 0u128;
    for _ in 0..10_000 {
        let mut ts = Timespec::default();
        assert_eq!(unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) }, 0);
        let ns = ts.as_ns();
        assert!(ns >= prev, "el reloj retrocedio: {} tras {}", ns, prev);
        prev = ns;
    }
}

/// The reason `vdso_tsc_to_ns` uses `unsigned __int128`.
///
/// `rdtsc()` cannot be chosen by the test, but `tsc_mult` can, and the product
/// is what overflows. A multiplier of 1000 ns/tick pushes `rdtsc() * mult` past
/// 2^64 on any machine whose TSC has been running for more than a few seconds,
/// so a 64-bit implementation wraps and returns a wildly wrong time — no crash,
/// no error, just a clock that is off by multiples of 2^64 ns. The test refuses
/// to pass vacuously: it first checks that the inputs really do overflow and
/// that wrapping arithmetic really would give a different answer.
#[test]
fn conversion_survives_an_overflowing_product() {
    const NS_PER_TICK: u64 = 1000;
    let mult = NS_PER_TICK << 32;

    let m = Mapping::new(linux_vdso::VdsoData {
        enabled: 1,
        _pad: 0,
        tsc_mult: mult,
        wall_off_ns: 0,
    });
    let clock_gettime: ClockGettime = m.sym("__vdso_clock_gettime");

    let before = rdtsc();
    let mut ts = Timespec::default();
    assert_eq!(unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) }, 0);
    let after = rdtsc();

    // The inputs must actually exercise the 128-bit path, or this proves
    // nothing. On a machine booted seconds ago they might not.
    assert!(
        before as u128 * mult as u128 > u64::MAX as u128,
        "el TSC del host ({}) es demasiado bajo para desbordar; el test no probaria nada",
        before
    );
    assert_ne!(
        before.wrapping_mul(mult) >> 32,
        ((before as u128 * mult as u128) >> 32) as u64,
        "aritmetica de 64 bits daria el mismo resultado: caso mal elegido"
    );

    let lo = (before as u128 * mult as u128) >> 32;
    let hi = (after as u128 * mult as u128) >> 32;
    assert!(
        ts.as_ns() >= lo && ts.as_ns() <= hi,
        "conversion incorrecta: {} fuera de [{}, {}]",
        ts.as_ns(),
        lo,
        hi
    );
    assert!(ts.tv_nsec >= 0 && ts.tv_nsec < 1_000_000_000);
}

#[test]
fn realtime_is_monotonic_plus_the_offset() {
    const OFFSET: u64 = 1_700_000_000_000_000_000; // ~2023 in ns since boot
    let m = Mapping::new(enabled(OFFSET));
    let clock_gettime: ClockGettime = m.sym("__vdso_clock_gettime");

    let mut mono = Timespec::default();
    let mut real = Timespec::default();
    assert_eq!(unsafe { clock_gettime(CLOCK_MONOTONIC, &mut mono) }, 0);
    assert_eq!(unsafe { clock_gettime(CLOCK_REALTIME, &mut real) }, 0);

    let delta = real.as_ns() - mono.as_ns();
    // The two reads are microseconds apart at most; the offset dominates.
    assert!(
        delta >= OFFSET as u128 && delta < OFFSET as u128 + 1_000_000_000,
        "realtime - monotonic = {}, se esperaba ~{}",
        delta,
        OFFSET
    );
    assert!(real.tv_nsec >= 0 && real.tv_nsec < 1_000_000_000);
}

/// Every unserved path must be one musl retries as a syscall.
///
/// musl returns `-EINVAL` straight to the caller and falls through on anything
/// else, so an unsupported clock reported as `-EINVAL` would turn "the vDSO
/// cannot do this one" into "this clock does not exist" for the whole process.
#[test]
fn unserved_paths_fall_back_instead_of_failing() {
    const EINVAL: i32 = -22;

    // Disabled by the kernel: nothing may be answered, including clocks the
    // image otherwise implements.
    let off = Mapping::new(linux_vdso::VdsoData::default());
    let clock_gettime: ClockGettime = off.sym("__vdso_clock_gettime");
    for clk in [CLOCK_REALTIME, CLOCK_MONOTONIC] {
        let mut ts = Timespec { tv_sec: -1, tv_nsec: -1 };
        let r = unsafe { clock_gettime(clk, &mut ts) };
        assert_eq!(r, ENOSYS, "con enabled=0 el reloj {} debe delegar al kernel", clk);
        assert_ne!(r, EINVAL, "-EINVAL romperia el reloj para todo el proceso");
        assert_eq!(ts, Timespec { tv_sec: -1, tv_nsec: -1 }, "no debe tocar el timespec");
    }

    // Enabled, but a clock this image does not implement.
    let on = Mapping::new(enabled(0));
    let clock_gettime: ClockGettime = on.sym("__vdso_clock_gettime");
    for clk in [CLOCK_PROCESS_CPUTIME_ID, 3, 7, 11, -1, 0x4000_0000] {
        let mut ts = Timespec::default();
        let r = unsafe { clock_gettime(clk, &mut ts) };
        assert_eq!(r, ENOSYS, "el reloj {} debe delegar al kernel", clk);
    }
}

#[test]
fn gettimeofday_and_time_agree_with_clock_gettime() {
    const OFFSET: u64 = 1_700_000_000_000_000_000;
    let m = Mapping::new(enabled(OFFSET));
    let clock_gettime: ClockGettime = m.sym("__vdso_clock_gettime");
    let gettimeofday: Gettimeofday = m.sym("__vdso_gettimeofday");
    let time: Time = m.sym("__vdso_time");

    let mut ts = Timespec::default();
    let mut tv = Timeval::default();
    let mut tz = Timezone { tz_minuteswest: 7, tz_dsttime: 7 };
    assert_eq!(unsafe { clock_gettime(CLOCK_REALTIME, &mut ts) }, 0);
    assert_eq!(unsafe { gettimeofday(&mut tv, &mut tz) }, 0);
    let mut t_out = 0i64;
    let t = unsafe { time(&mut t_out) };

    assert_eq!(tz.tz_minuteswest, 0, "gettimeofday debe rellenar la zona horaria");
    assert_eq!(tz.tz_dsttime, 0);
    assert_eq!(tv.tv_sec, ts.tv_sec, "gettimeofday y clock_gettime discrepan");
    assert!(tv.tv_usec >= 0 && tv.tv_usec < 1_000_000, "tv_usec fuera de rango: {}", tv.tv_usec);
    assert_eq!(t, ts.tv_sec, "time() y clock_gettime discrepan");
    assert_eq!(t_out, t, "time() no escribio su argumento");

    // A NULL argument is legal for both and must not fault.
    assert_eq!(unsafe { gettimeofday(&mut tv, std::ptr::null_mut()) }, 0);
    assert_eq!(unsafe { time(std::ptr::null_mut()) }, t);
    assert_eq!(unsafe { gettimeofday(std::ptr::null_mut(), &mut tz) }, 0);
}

/// The image must run correctly wherever it lands, since every process maps it
/// at a different address and nothing relocates it.
#[test]
fn works_from_several_load_addresses() {
    let mappings: Vec<Mapping> = (0..8).map(|_| Mapping::new(enabled(0))).collect();
    let bases: Vec<usize> = mappings.iter().map(|m| m.base as usize).collect();
    assert!(
        bases.iter().collect::<std::collections::HashSet<_>>().len() > 1,
        "se esperaban direcciones de carga distintas"
    );
    for m in &mappings {
        let clock_gettime: ClockGettime = m.sym("__vdso_clock_gettime");
        let mut ts = Timespec::default();
        assert_eq!(unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) }, 0);
        assert!(ts.as_ns() > 0);
    }
}
