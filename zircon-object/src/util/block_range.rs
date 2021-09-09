#![allow(dead_code)]

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
    pub fn is_full(&self) -> bool {
        self.len() == (1usize << self.block_size_log2)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
