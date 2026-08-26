use crate::utils::init_once::InitOnce;

pub use super::imp::config::KernelConfig;

#[cfg(feature = "libos")]
pub(crate) static KCONFIG: InitOnce<KernelConfig> = InitOnce::new_with_default(KernelConfig);

#[cfg(not(feature = "libos"))]
pub(crate) static KCONFIG: InitOnce<KernelConfig> = InitOnce::new();

/// Número máximo de CPUs (id lógico denso 0..MAX_CORE_NUM).
///
/// Debe ser <= al `MAX_CORE_NUM` interno del crate `lock` (vendor/kernel-sync),
/// que dimensiona su array per-CPU indexado por el mismo id lógico.
pub const MAX_CORE_NUM: usize = 64;
