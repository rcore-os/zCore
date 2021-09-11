use crate::utils::init_once::InitOnce;

pub use super::imp::config::KernelConfig;

#[allow(dead_code)]
pub(crate) static KCONFIG: InitOnce<KernelConfig> = InitOnce::new();
