use spin::Once;

/// Configuration of HAL.
pub struct HalConfig {
    pub acpi_rsdp: u64,
    pub smbios: u64,
    pub ap_fn: fn() -> !,
}

pub(super) static CONFIG: Once<HalConfig> = Once::new();
