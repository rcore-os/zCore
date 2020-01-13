use super::*;

impl Syscall {
    pub fn sys_debug_write(&self, buf: UserInPtr<u8>, len: usize) -> ZxResult<usize> {
        let data = buf.read_array(len)?;
        for c in data {
            serial_write(c as char);
        }
        serial_write('\n');
        Ok(0)
    }
}
