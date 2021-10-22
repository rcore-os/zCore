use crate::utils::init_once::InitOnce;

pub use super::imp::config::KernelConfig;

#[cfg(feature = "libos")]
pub(crate) static KCONFIG: InitOnce<KernelConfig> = InitOnce::new_with_default(KernelConfig);

#[cfg(not(feature = "libos"))]
pub(crate) static KCONFIG: InitOnce<KernelConfig> = InitOnce::new();
