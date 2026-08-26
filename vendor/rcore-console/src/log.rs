//! Dummy macros for log disabled
#![allow(unused_macros)]

macro_rules! trace {
    ($($arg:expr),*) => ({ $( let _ = $arg; )* });
}
macro_rules! debug {
    ($($arg:expr),*) => ({ $( let _ = $arg; )* });
}
macro_rules! info {
    ($($arg:expr),*) => ({ $( let _ = $arg; )* });
}
macro_rules! warn {
    ($($arg:expr),*) => ({ $( let _ = $arg; )* });
}
macro_rules! error {
    ($($arg:expr),*) => ({ $( let _ = $arg; )* });
}
