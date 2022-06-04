#[cfg(target_arch = "riscv64")]
fn main() {}

#[cfg(not(target_arch = "riscv64"))]
fn main() -> shadow_rs::SdResult<()> {
    shadow_rs::new()
}
