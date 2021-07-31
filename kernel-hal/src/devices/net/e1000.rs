use alloc::string::String;

#[linkage = "weak"]
#[export_name = "hal_net_e1000_init"]
pub fn init(_name: String, _irq: Option<usize>, _header: usize, _size: usize, _index: usize) {}
