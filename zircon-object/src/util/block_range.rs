/// Given a range and iterate sub-range for each block
#[derive(Debug)]
pub struct BlockIter {
    pub begin: usize,
    pub end: usize,
    pub block_size_log2: u8,
}

#[derive(Debug, Eq, PartialEq)]
pub struct BlockRange {
    pub block: usize,
    pub begin: usize,
    pub end: usize,
    pub block_size_log2: u8,
}

impl BlockRange {
    pub fn len(&self) -> usize {
        self.end - self.begin
    }
    pub fn origin_begin(&self) -> usize {
        (self.block << self.block_size_log2) + self.begin
    }
    pub fn origin_end(&self) -> usize {
        (self.block << self.block_size_log2) + self.end
    }
}

impl Iterator for BlockIter {
    type Item = BlockRange;

    fn next(&mut self) -> Option<<Self as Iterator>::Item> {
        if self.begin >= self.end {
            return None;
        }
        let block_size_log2 = self.block_size_log2;
        let block_size = 1usize << self.block_size_log2;
        let block = self.begin / block_size;
        let begin = self.begin % block_size;
        let end = if block == self.end / block_size {
            self.end % block_size
        } else {
            block_size
        };
        self.begin += end - begin;
        Some(BlockRange {
            block,
            begin,
            end,
            block_size_log2,
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    fn collect(begin: usize, end: usize, log2: u8) -> Vec<BlockRange> {
        BlockIter {
            begin,
            end,
            block_size_log2: log2,
        }
        .collect()
    }

    #[test]
    fn empty_range_yields_nothing() {
        assert!(collect(0, 0, 12).is_empty());
        assert!(collect(0x500, 0x500, 12).is_empty());
        // end < begin is treated as empty too.
        assert!(collect(0x800, 0x100, 12).is_empty());
    }

    #[test]
    fn within_single_block() {
        assert_eq!(
            collect(0x10, 0x20, 12),
            vec![BlockRange {
                block: 0,
                begin: 0x10,
                end: 0x20,
                block_size_log2: 12
            }]
        );
        // A sub-range fully inside block 3.
        assert_eq!(
            collect(0x3100, 0x3900, 12),
            vec![BlockRange {
                block: 3,
                begin: 0x100,
                end: 0x900,
                block_size_log2: 12
            }]
        );
    }

    #[test]
    fn exactly_one_full_block() {
        assert_eq!(
            collect(0x1000, 0x2000, 12),
            vec![BlockRange {
                block: 1,
                begin: 0,
                end: 0x1000,
                block_size_log2: 12
            }]
        );
    }

    #[test]
    fn aligned_multi_block() {
        let v = collect(0, 0x3000, 12);
        assert_eq!(v.len(), 3);
        for (i, r) in v.iter().enumerate() {
            assert_eq!(
                *r,
                BlockRange {
                    block: i,
                    begin: 0,
                    end: 0x1000,
                    block_size_log2: 12
                }
            );
        }
    }

    #[test]
    fn ends_on_block_boundary() {
        // Starts mid-block-0, ends exactly at the end of block 1.
        assert_eq!(
            collect(0x800, 0x2000, 12),
            vec![
                BlockRange {
                    block: 0,
                    begin: 0x800,
                    end: 0x1000,
                    block_size_log2: 12
                },
                BlockRange {
                    block: 1,
                    begin: 0,
                    end: 0x1000,
                    block_size_log2: 12
                },
            ]
        );
    }

    #[test]
    fn smaller_block_size() {
        // 512-byte blocks (log2 = 9): bytes 0x100..0x500 span blocks 0, 1, 2.
        assert_eq!(
            collect(0x100, 0x500, 9),
            vec![
                BlockRange {
                    block: 0,
                    begin: 0x100,
                    end: 0x200,
                    block_size_log2: 9
                },
                BlockRange {
                    block: 1,
                    begin: 0,
                    end: 0x200,
                    block_size_log2: 9
                },
                BlockRange {
                    block: 2,
                    begin: 0,
                    end: 0x100,
                    block_size_log2: 9
                },
            ]
        );
    }

    #[test]
    fn len_and_origin_helpers() {
        let r = BlockRange {
            block: 2,
            begin: 0x100,
            end: 0x180,
            block_size_log2: 12,
        };
        assert_eq!(r.len(), 0x80);
        assert_eq!(r.origin_begin(), (2 << 12) + 0x100);
        assert_eq!(r.origin_end(), (2 << 12) + 0x180);
    }

    // The fundamental invariants the iterator must always satisfy: every yielded
    // sub-range stays inside its block, the pieces are contiguous in the
    // original address space, and their lengths sum to the requested span.
    #[test]
    fn invariants_hold_for_many_ranges() {
        let block_size: usize = 1 << 12;
        let cases = [
            (0usize, 0x10usize),
            (0x123, 0x2018),
            (0, 0x4000),
            (0x1, 0x4001),
            (0xfff, 0x1001),
        ];
        for &(begin, end) in &cases {
            let mut expected_origin = begin;
            let mut total = 0usize;
            for r in (BlockIter {
                begin,
                end,
                block_size_log2: 12,
            }) {
                assert!(r.begin < block_size);
                assert!(r.end <= block_size);
                assert!(r.begin < r.end, "each piece is non-empty");
                assert_eq!(r.origin_begin(), expected_origin, "pieces are contiguous");
                total += r.len();
                expected_origin = r.origin_end();
            }
            assert_eq!(expected_origin, end, "covers up to end");
            assert_eq!(total, end - begin, "lengths sum to the span");
        }
    }

    #[test]
    fn block_iter() {
        let mut iter = BlockIter {
            begin: 0x123,
            end: 0x2018,
            block_size_log2: 12,
        };
        assert_eq!(
            iter.next(),
            Some(BlockRange {
                block: 0,
                begin: 0x123,
                end: 0x1000,
                block_size_log2: 12
            })
        );
        assert_eq!(
            iter.next(),
            Some(BlockRange {
                block: 1,
                begin: 0,
                end: 0x1000,
                block_size_log2: 12
            })
        );
        assert_eq!(
            iter.next(),
            Some(BlockRange {
                block: 2,
                begin: 0,
                end: 0x18,
                block_size_log2: 12
            })
        );
        assert_eq!(iter.next(), None);
    }
}
