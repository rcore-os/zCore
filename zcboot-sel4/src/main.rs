#![no_std]
#![no_main]

#[no_mangle]
pub unsafe extern "C" fn rust_start() -> ! {
    kernel_hal_sel4::boot()
}

#[no_mangle]
extern "C" fn zircon_start() {
    use zircon_loader::{run_userboot, Images};
    let images = Images::<&[u8]> {
        userboot: include_bytes!("../../prebuilt/zircon/x64/userboot.so"),
        vdso: include_bytes!("../../prebuilt/zircon/x64/libzircon.so"),
        zbi: include_bytes!("../../prebuilt/zircon/x64/bringup.zbi"),
    };
    run_userboot(&images, "");
}