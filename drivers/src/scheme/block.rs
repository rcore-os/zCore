use super::Scheme;
use crate::DeviceResult;

/// Block device interface.
///
/// Convention shared by all implementations (AHCI, NVMe, partitions):
/// `block_id` indexes 512-byte sectors and `buf.len()` must be a non-zero
/// multiple of 512. A single call may transfer many sectors; drivers split
/// the request internally as needed.
pub trait BlockScheme: Scheme {
    fn read_block(&self, block_id: usize, buf: &mut [u8]) -> DeviceResult;
    fn write_block(&self, block_id: usize, buf: &[u8]) -> DeviceResult;
    fn flush(&self) -> DeviceResult;
    /// Total capacity in 512-byte sectors.
    fn block_count(&self) -> usize;
    /// Prepare the device for a warm reset / power-off: flush volatile write
    /// caches and, where the protocol defines one (NVMe CC.SHN), perform an
    /// orderly shutdown. DRAM-less SSDs persist their FTL mapping tables on
    /// this signal, so skipping it across a warm reset costs them a recovery
    /// scan (and, on marginal firmware, risks mapping-table damage). Must be
    /// best-effort and time-bounded — it runs on the reboot path.
    fn quiesce_for_reboot(&self) {
        let _ = self.flush();
    }
}
