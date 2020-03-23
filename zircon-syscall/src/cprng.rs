use {super::*, core::arch::x86_64::_rdrand32_step};

#[allow(unsafe_code)]
impl Syscall<'_> {
    pub fn sys_cprng_draw_once(&self, buf: usize, len: usize) -> ZxResult<usize> {
        info!("cprng_draw_once: buf=({:#x}; {:?})", buf, len);
        if len % 4 == 0 {
            let size = len / 4;
            let mut res = vec![0u32; size];
            res.iter_mut().for_each(|value| unsafe {
                // TODO: move to HAL
                _rdrand32_step(value);
            });
            UserOutPtr::<u32>::from(buf).write_array(&res)?;
            Ok(len)
        } else {
            unimplemented!()
        }
    }
}
