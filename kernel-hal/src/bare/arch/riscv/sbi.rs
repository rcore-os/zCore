#![allow(dead_code)]
// Legacy Extensions (EIDs 0x00 - 0x0F)
const SBI_SET_TIMER: usize = 0;
const SBI_CONSOLE_PUTCHAR: usize = 1;
const SBI_CONSOLE_GETCHAR: usize = 2;
const SBI_CLEAR_IPI: usize = 3;
const SBI_SEND_IPI: usize = 4;
const SBI_REMOTE_FENCE_I: usize = 5;
const SBI_REMOTE_SFENCE_VMA: usize = 6;
const SBI_REMOTE_SFENCE_VMA_ASID: usize = 7;
const SBI_SHUTDOWN: usize = 8;

//  Hart State Management Extension
const HSM_EID: usize = 0x48534D;
const SBI_HART_START_FID: usize = 0; // SBI Verson=0.2
const SBI_HART_STOP_FID: usize = 1; // SBI Verson=0.2
const SBI_HART_GET_STATUS_FID: usize = 2; // SBI Verson=0.2
const SBI_HART_SUSPEND_FID: usize = 3; // SBI Verson=0.3

// SBI Error Code
pub const SBI_SUCCESS: usize = 0;
pub const SBI_ERR_FAILED: usize = usize::MAX; // -1
pub const SBI_ERR_NOT_SUPPORTED: usize = usize::MAX - 1; // -2
pub const SBI_ERR_INVALID_PARAM: usize = usize::MAX - 2; // -3
pub const SBI_ERR_DENIED: usize = usize::MAX - 3; // -4
pub const SBI_ERR_INVALID_ADDRESS: usize = usize::MAX - 4; // -5
pub const SBI_ERR_ALREADY_AVAILABLE: usize = usize::MAX - 5; // -6
pub const SBI_ERR_ALREADY_STARTED: usize = usize::MAX - 6; // -7
pub const SBI_ERR_ALREADY_STOPPED: usize = usize::MAX - 7; // -8

#[inline(always)]
fn sbi_call(eid: usize, fid: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
    let ret;
    unsafe {
        asm!("ecall",
            in("a0") arg0,
            in("a1") arg1,
            in("a2") arg2,
            in("a6") fid,
            in("a7") eid,
            lateout("a0") ret,
        );
    }
    ret
}

pub fn console_putchar(ch: usize) -> usize {
    sbi_call(SBI_CONSOLE_PUTCHAR, 0, ch, 0, 0)
}

pub fn console_getchar() -> usize {
    sbi_call(SBI_CONSOLE_GETCHAR, 0, 0, 0, 0)
}

pub fn set_timer(stime_value: u64) -> usize {
    #[cfg(target_pointer_width = "32")]
    return sbi_call(
        SBI_SET_TIMER,
        0,
        stime_value as usize,
        (stime_value >> 32),
        0,
    );

    #[cfg(target_pointer_width = "64")]
    sbi_call(SBI_SET_TIMER, 0, stime_value as usize, 0, 0)
}

pub fn clear_ipi() -> usize {
    sbi_call(SBI_CLEAR_IPI, 0, 0, 0, 0)
}

pub fn send_ipi(sipi_value: usize) -> usize {
    sbi_call(SBI_SEND_IPI, 0, sipi_value, 0, 0)
}

/// executing the target hart in supervisor-mode at address
/// specified by start_addr parameter
///
/// The opaque parameter is a XLEN-bit value which will be
/// set in the a1 register when the hart starts executing
/// at start_addr.
pub fn hart_start(hartid: usize, start_addr: usize, opaque: usize) -> usize {
    sbi_call(HSM_EID, SBI_HART_START_FID, hartid, start_addr, opaque)
}

/// stop executing the calling hart in supervisor-mode and return
/// it’s ownership to the SBI implementation.
pub fn hart_stop() -> usize {
    sbi_call(HSM_EID, SBI_HART_STOP_FID, 0, 0, 0)
}

hal_fn_impl! {
    impl mod crate::hal_fn::console {
        fn console_write_early(s: &str) {
            for c in s.bytes() {
                console_putchar(c as usize);
            }
        }
    }
}
