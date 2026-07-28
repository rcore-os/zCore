//! CPU information.

use raw_cpuid::CpuId;

/// PIT (8254) channel 0 reference frequency in Hz. Fixed by the spec — every
/// x86 PC (and QEMU) clocks the PIT at 1.193182 MHz.
const PIT_REF_HZ: u64 = 1_193_182;

/// Measure the TSC frequency by counting TSC cycles while the PIT channel 2
/// counts down a known number of ticks. Channel 2 is the speaker channel and
/// is not used by the kernel timer (we use the LAPIC timer), so there is no
/// conflict with the system tick.
///
/// SAFETY: touches the legacy 8254/0x61 ports; must only be called from a
/// single core early in boot, before any other code uses the PIT.
unsafe fn calibrate_tsc_mhz_via_pit() -> Option<u16> {
    use x86_64::instructions::port::Port;

    // ~54.9 ms gate window (65535 / 1.193182 MHz). Long enough that IRQ
    // jitter is irrelevant; short enough that a slow VM still finishes.
    const PIT_COUNT: u16 = 0xFFFF;

    let mut gate = Port::<u8>::new(0x61);
    let mut cmd = Port::<u8>::new(0x43);
    let mut data = Port::<u8>::new(0x42);

    let saved = gate.read();
    // Speaker off (bit 1 = 0), gate low (bit 0 = 0).
    gate.write(saved & 0xFC);

    // Channel 2, access lo+hi, mode 0 (interrupt on terminal count), binary.
    cmd.write(0b1011_0000);
    data.write((PIT_COUNT & 0xFF) as u8);
    data.write((PIT_COUNT >> 8) as u8);

    // Raise gate → counter starts decrementing on the next PIT tick.
    let t0 = core::arch::x86_64::_rdtsc();
    gate.write((saved & 0xFC) | 0x01);

    // Mode 0: OUT2 (bit 5 of 0x61) stays low until the counter hits zero,
    // then goes high. Spin-poll with a safety cap so a missing PIT doesn't
    // hang the kernel — at 1 read per ~hundreds of cycles, 2 billion
    // iterations covers any plausible CPU running well past the 55 ms window.
    let mut spins: u64 = 0;
    while gate.read() & 0x20 == 0 {
        spins = spins.wrapping_add(1);
        if spins > 2_000_000_000 {
            // PIT not counting — restore and bail.
            gate.write(saved);
            return None;
        }
        core::hint::spin_loop();
    }
    let t1 = core::arch::x86_64::_rdtsc();
    gate.write(saved);

    let cycles = t1.saturating_sub(t0);
    // hz = cycles * PIT_REF_HZ / PIT_COUNT
    let hz = cycles.saturating_mul(PIT_REF_HZ) / PIT_COUNT as u64;
    let mhz = hz / 1_000_000;
    if (100..=20_000).contains(&mhz) {
        Some(mhz as u16)
    } else {
        None
    }
}

hal_fn_impl! {
    impl mod crate::hal_fn::cpu {
        fn cpu_id() -> u8 {
            // Dense logical CPU id (0..NCPU), resolved from the sparse Local APIC
            // ID through the table populated during SMP bring-up (see `smp.rs`).
            // The raw APIC ID must NOT be used to index per-CPU arrays: it is not
            // contiguous and can exceed the CPU count, causing out-of-bounds
            // panics. `lock` owns the apic->logical map so the kernel and the lock
            // crate agree on a single id space.
            lock::current_cpu_id()
        }

        fn cpu_frequency() -> u16 {
            static CPU_FREQ_MHZ: spin::Once<u16> = spin::Once::new();
            *CPU_FREQ_MHZ.call_once(|| {
                // Prefer measuring the TSC directly against the PIT: on modern
                // Intel the TSC runs at the nominal (non-turbo) frequency,
                // which is NOT the same as CPUID's "base frequency"; on AMD
                // and on QEMU guests CPUID leaf 0x16 is absent altogether.
                // Without calibration the kernel clock ran ~1.8× too fast,
                // collapsing TCP RTOs and inflating uptime on real hardware.
                if let Some(mhz) = unsafe { calibrate_tsc_mhz_via_pit() } {
                    return mhz;
                }
                // Fallback chain: CPUID base frequency, then a conservative
                // default. Both are wrong on modern boxes but avoid div-by-0.
                const DEFAULT: u16 = 2000;
                CpuId::new()
                    .get_processor_frequency_info()
                    .map(|info| info.processor_base_frequency())
                    .filter(|&f| f >= 100)
                    .unwrap_or(DEFAULT)
            })
        }

        fn cpu_brand() -> alloc::string::String {
            use core::arch::x86_64::__cpuid;
            let mut brand = alloc::vec::Vec::new();
            for leaf in 0x80000002..=0x80000004 {
                let res = __cpuid(leaf);
                for reg in &[res.eax, res.ebx, res.ecx, res.edx] {
                    brand.extend_from_slice(&reg.to_le_bytes());
                }
            }
            let brand_str = core::str::from_utf8(&brand)
                .unwrap_or("")
                .trim_matches('\0')
                .trim();
            alloc::string::String::from(brand_str)
        }

        fn cpu_count() -> u8 {
            super::smp::CPU_COUNT.load(core::sync::atomic::Ordering::Acquire) as u8
        }

        fn cpu_temperature_mc() -> Option<i32> {
            super::power::cpu_temperature_mc()
        }

        fn pstate_governor_summary() -> Option<(u32, u8, u8)> {
            super::power::governor_summary()
        }

        fn reset() -> ! {
            info!("resetting/shutting down...");
            use zcore_drivers::io::{Io, Pmio};

            // Leave any GPU we brought up in a clean (cold) state first. This is
            // a WARM reset (it does not power-cycle PCIe devices), so a GPU with
            // a live GSP-RM / locked WPR2 would otherwise cross the reboot dirty
            // and stall the firmware POST that runs before the OS next boot.
            // Best-effort and bounded (FLR + 100 ms per brought-up GPU); the
            // console GPU and non-NVIDIA drivers are no-ops.
            for d in crate::drivers::all_drm().as_vec().iter() {
                let _ = d.quiesce_for_reboot();
            }

            // Method 1: PS/2 Controller (Keyboard Controller)
            // Writing 0xFE to port 0x64 triggers a pulse on the reset line.
            Pmio::<u8>::new(0x64).write(0xFE);

            // Method 2: PCI Reset Control Register (standard on many chipsets)
            // Port 0xCF9. 0x06 = system reset, 0x0E = hard reset.
            Pmio::<u8>::new(0xCF9).write(0x06);
            Pmio::<u8>::new(0xCF9).write(0x0E);

            // Method 3: QEMU/ACPI Poweroff (fallback for halt/poweroff)
            Pmio::<u16>::new(0x604).write(0x2000);

            // Method 4: Triple Fault (the "nuclear" option)
            // Load a zero-length IDT and trigger an interrupt.
            unsafe {
                let idtr: [u16; 5] = [0, 0, 0, 0, 0];
                core::arch::asm!("lidt [{}]", in(reg) &idtr);
                core::arch::asm!("int3");
            }

            loop {
                super::interrupt::wait_for_interrupt();
            }
        }
    }
}
