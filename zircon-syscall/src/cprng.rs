use {super::*, core::arch::x86_64::_rdrand64_step};

#[allow(unsafe_code)]
impl Syscall {
    pub fn sys_cprng_draw_once(&self, mut buf: UserOutPtr<u64>, _len: usize) -> ZxResult<usize> {
        let mut res = 0u64;
        if unsafe { _rdrand64_step(&mut res) } == 1 {
            buf.write(res)?;
            Ok(0)
        } else {
            Err(ZxError::INVALID_ARGS)
        }
    }
}
