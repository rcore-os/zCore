use crate::config::KernelConfig;
use spin::Once;

#[used]
pub(crate) static CONFIG: Once<KernelConfig> = Once::new();
