use alloc::format;

use kernel_hal::{BlockDriver, Driver};
use nezha_sdc::{MmcHost, sdcard_init};
use kernel_hal::drivers::DeviceType;
use super::{BLK_DRIVERS, net::realtek::mii::MII_MMD_CTRL};
use alloc::string::String;
use alloc::sync::Arc;
pub fn sdc_init(){
    nezha_sdc::sdcard_init();
    BLK_DRIVERS.write().push(Arc::new(SDCARD));
}
struct SDCARD;
impl BlockDriver for SDCARD {
    fn read_block(&self, block_id: usize, buf: &mut [u8]) -> bool {
        let mut cmd = MmcHost::new();
        cmd.set_data(buf.as_ptr() as *const _ as usize);
        unsafe{
            cmd.read_block(block_id as u32, buf.len() as u32 / 512);
        }
        true
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) -> bool {
        let mut cmd = MmcHost::new();
        cmd.set_data(buf.as_ptr() as *const _ as usize);
        unsafe{
            cmd.write_block(block_id as u32, buf.len() as u32 / 512);
        }
        true
    }
}
impl Driver for SDCARD{
    fn try_handle_interrupt(&self, _irq: Option<usize>) -> bool {
        warn!("SDCARD HANDEL INTERRUPT+++++++");
        false
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn get_id(&self) -> String {
        format!("SD_CARD")
    }

    fn as_block(&self) -> Option<&dyn BlockDriver> {
        Some(self)
    }
}