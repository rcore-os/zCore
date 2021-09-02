/// Fill random bytes to the buffer
#[allow(unused_variables)]
pub fn fill_random(buf: &mut [u8]) {
    cfg_if::cfg_if! {
        if #[cfg(target_arch = "x86_64")] {
            // TODO: optimize
            for x in buf.iter_mut() {
                let mut r = 0;
                unsafe {
                    core::arch::x86_64::_rdrand16_step(&mut r);
                }
                *x = r as _;
            }
        } else {
            unimplemented!()
        }
    }
}
