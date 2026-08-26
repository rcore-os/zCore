use crate::cell::Cell;
use crate::text_buffer::TextBuffer;
use alloc::vec::Vec;

/// Cache layer for [`TextBuffer`]
pub struct TextBufferCache<T: TextBuffer> {
    buf: Vec<Vec<Cell>>,
    row_offset: usize,
    inner: T,
}

impl<T: TextBuffer> TextBufferCache<T> {
    /// Create a cache layer for `inner` text buffer
    pub fn new(inner: T) -> Self {
        TextBufferCache {
            buf: vec![vec![Cell::default(); inner.width()]; inner.height()],
            row_offset: 0,
            inner,
        }
    }
    /// Get real row of inner buffer
    fn real_row(&self, row: usize) -> usize {
        (self.row_offset + row) % self.inner.height()
    }
    /// Clear line at `row`
    fn clear_line(&mut self, row: usize, cell: Cell) {
        for col in 0..self.width() {
            self.buf[row][col] = cell;
            self.inner.write(row, col, cell);
        }
    }
}

impl<T: TextBuffer> TextBuffer for TextBufferCache<T> {
    #[inline]
    fn width(&self) -> usize {
        self.inner.width()
    }

    #[inline]
    fn height(&self) -> usize {
        self.inner.height()
    }

    #[inline]
    fn read(&self, row: usize, col: usize) -> Cell {
        let row = self.real_row(row);
        self.buf[row][col]
    }

    #[inline]
    fn write(&mut self, row: usize, col: usize, cell: Cell) {
        let row = self.real_row(row);
        self.buf[row][col] = cell;
        self.inner.write(row, col, cell);
    }

    #[inline]
    fn new_line(&mut self, cell: Cell) {
        self.clear_line(self.row_offset, cell);
        self.row_offset = (self.row_offset + 1) % self.inner.height();
    }

    #[inline]
    fn clear(&mut self, cell: Cell) {
        self.row_offset = 0;
        self.inner.clear(cell);
    }
}
