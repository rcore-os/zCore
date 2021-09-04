use lazy_static::lazy_static;
use std::collections::VecDeque;
use std::sync::Mutex;

lazy_static! {
    static ref STDIN: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
    static ref STDIN_CALLBACK: Mutex<Vec<Box<dyn Fn() -> bool + Send + Sync>>> =
        Mutex::new(Vec::new());
}

hal_fn_impl! {
    impl mod crate::defs::serial {
        fn serial_put(x: u8) {
            STDIN.lock().unwrap().push_back(x);
            STDIN_CALLBACK.lock().unwrap().retain(|f| !f());
        }

        fn serial_set_callback(callback: Box<dyn Fn() -> bool + Send + Sync>) {
            STDIN_CALLBACK.lock().unwrap().push(callback);
        }

        fn serial_read(buf: &mut [u8]) -> usize {
            let mut stdin = STDIN.lock().unwrap();
            let len = stdin.len().min(buf.len());
            for c in &mut buf[..len] {
                *c = stdin.pop_front().unwrap();
            }
            len
        }

        fn print_fmt(fmt: core::fmt::Arguments) {
            eprint!("{}", fmt);
        }
    }
}
