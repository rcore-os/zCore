//use super::*;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TimeSpec {
    sec: usize,
    nsec: usize,
}
