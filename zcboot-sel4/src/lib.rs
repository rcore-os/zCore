#![no_std]

#[no_mangle]
pub unsafe extern "C" fn rust_start() -> ! {
    kernel_hal_sel4::boot()
}
