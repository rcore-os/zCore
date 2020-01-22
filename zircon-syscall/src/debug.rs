use super::*;
use zircon_object::hal;

impl Syscall {
    pub fn sys_debug_write(&self, buf: UserInPtr<u8>, len: usize) -> ZxResult<usize> {
        let data = buf.read_array(len)?;
        for c in data {
            hal::serial_write(c as char);
        }
        hal::serial_write('\n');
        Ok(0)
    }
}
