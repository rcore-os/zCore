mod random;

pub use random::RandomINode;

cfg_if::cfg_if! {
    if #[cfg(feature = "graphic")] {
        mod fbdev;
        // mod input;
        // pub use self::input::*;
        pub use self::fbdev::Fbdev;
    }
}
