use super::*;

impl Syscall<'_> {
    pub fn sys_cprng_draw_once(&self, mut buf: UserOutPtr<u8>, len: usize) -> ZxResult {
        info!("cprng_draw_once: buf=({:?}; {:?})", buf, len);
        let mut res = vec![0u8; len];
        kernel_hal::fill_random(&mut res);
        buf.write_array(&res)?;
        Ok(())
    }
}
