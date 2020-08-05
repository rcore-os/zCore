extern "C" {
    fn l4bridge_debug_print(s: *const u8, len: usize);
}

#[no_mangle]
pub unsafe extern "C" fn rust_start() -> ! {
    let s = "Hello from zCore!".as_bytes();
    l4bridge_debug_print(s.as_ptr(), s.len());
    loop {}
}
