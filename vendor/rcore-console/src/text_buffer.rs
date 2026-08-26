use crate::cell::Cell;

/// A 2D array of `Cell` to render on screen
pub trait TextBuffer {
    /// Columns
    fn width(&self) -> usize;

    /// Rows
    fn height(&self) -> usize;

    /// Read the character at `(row, col)`
    ///
    /// Avoid use this because it's usually very slow on real hardware.
    fn read(&self, row: usize, col: usize) -> Cell;

    /// Write a character `ch` at `(row, col)`
    fn write(&mut self, row: usize, col: usize, cell: Cell);

    /// Delete one character at `(row, col)`.
    fn delete(&mut self, row: usize, col: usize) {
        self.write(row, col, Cell::default());
    }

    /// Insert one blank line at the bottom, and scroll up one line.
    ///
    /// The default method does single read and write for each pixel.
    /// Usually it needs rewrite for better performance.
    fn new_line(&mut self, cell: Cell) {
        for i in 1..self.height() {
            for j in 0..self.width() {
                self.write(i - 1, j, self.read(i, j));
            }
        }
        for j in 0..self.width() {
            self.write(self.height() - 1, j, cell);
        }
    }

    /// Clear the buffer
    fn clear(&mut self, cell: Cell) {
        for i in 0..self.height() {
            for j in 0..self.width() {
                self.write(i, j, cell);
            }
        }
    }

    /// Scroll rows `top..=bottom` (inclusive) up by `n` lines; the vacated rows
    /// at the bottom of the region are filled with `blank`.
    ///
    /// The default moves cells one by one. A framebuffer-backed implementation
    /// should override this to bulk-copy the corresponding pixel band (much
    /// faster for full-screen TUIs that scroll a sub-region).
    fn scroll_region_up(&mut self, top: usize, bottom: usize, n: usize, blank: Cell) {
        if top > bottom || bottom >= self.height() {
            return;
        }
        let n = n.min(bottom - top + 1);
        let width = self.width();
        for r in top..=bottom {
            for c in 0..width {
                let cell = if r + n <= bottom {
                    self.read(r + n, c)
                } else {
                    blank
                };
                self.write(r, c, cell);
            }
        }
    }

    /// Scroll rows `top..=bottom` (inclusive) down by `n` lines; the vacated
    /// rows at the top of the region are filled with `blank`.
    fn scroll_region_down(&mut self, top: usize, bottom: usize, n: usize, blank: Cell) {
        if top > bottom || bottom >= self.height() {
            return;
        }
        let n = n.min(bottom - top + 1);
        let width = self.width();
        for r in (top..=bottom).rev() {
            for c in 0..width {
                let cell = if r >= top + n {
                    self.read(r - n, c)
                } else {
                    blank
                };
                self.write(r, c, cell);
            }
        }
    }
}
