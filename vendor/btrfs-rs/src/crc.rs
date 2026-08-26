//! CRC32C (Castagnoli) used by btrfs for tree-block / superblock checksums
//! and for directory name hashes.

const POLY: u32 = 0x82F6_3B78; // reflected 0x1EDC6F41

const fn make_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ POLY
            } else {
                crc >> 1
            };
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static TABLE: [u32; 256] = make_table();

/// Raw CRC32C update without initial/final inversion (matches the kernel's
/// `crc32c(seed, data, len)`).
pub fn crc32c_update(seed: u32, data: &[u8]) -> u32 {
    let mut crc = seed;
    for &b in data {
        crc = (crc >> 8) ^ TABLE[((crc ^ b as u32) & 0xff) as usize];
    }
    crc
}

/// Standard CRC32C (init `!0`, final inversion) — what btrfs stores in
/// superblock and tree-block checksum fields.
pub fn checksum(data: &[u8]) -> u32 {
    !crc32c_update(!0, data)
}

/// btrfs directory entry name hash: `crc32c((u32)~1, name, len)` with no
/// final inversion.
pub fn name_hash(name: &[u8]) -> u64 {
    crc32c_update(!1u32, name) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // iSCSI CRC32C of "123456789" is 0xE3069283.
        assert_eq!(checksum(b"123456789"), 0xE306_9283);
        // btrfs name hash of "default" (as used by mkfs for the default
        // subvolume dir item) is 0x8dbfc2d2.
        assert_eq!(name_hash(b"default"), 0x8dbf_c2d2);
    }
}
