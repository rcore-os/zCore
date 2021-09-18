use std::os::unix::io::AsRawFd;
use std::sync::Mutex;

type MouseCallbackFn = dyn Fn([u8; 3]) + Send + Sync;
type KBDCallbackFn = dyn Fn(u16, i32) + Send + Sync;

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct InputEvent {
    time: TimeVal,
    type_: u16,
    code: u16,
    value: i32,
}

lazy_static! {
    static ref MOUSE_CALLBACK: Mutex<Vec<Box<MouseCallbackFn>>> = Mutex::new(Vec::new());
    static ref KBD_CALLBACK: Mutex<Vec<Box<KBDCallbackFn>>> = Mutex::new(Vec::new());
}

fn init_kbd() {
    let fd = std::fs::File::open("/dev/input/event1").expect("Failed to open input event device.");
    // ??
    /* let inputfd = unsafe {
        libc::open(
            "/dev/input/event1".as_ptr() as *const i8,
            libc::O_RDONLY /* | libc::O_NONBLOCK */,
        )
    }; */
    if fd.as_raw_fd() < 0 {
        return;
    }

    std::thread::spawn(move || {
        use core::mem::{size_of, transmute, transmute_copy};
        let ev = InputEvent::default();
        const LEN: usize = size_of::<InputEvent>();
        let mut buf: [u8; LEN] = unsafe { transmute(ev) };
        loop {
            std::thread::sleep(std::time::Duration::from_millis(8));
            let ret =
                unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, LEN) };
            if ret < 0 {
                break;
            }
            let ev: InputEvent = unsafe { transmute_copy(&buf) };
            if ev.type_ == 1 {
                KBD_CALLBACK.lock().unwrap().iter().for_each(|callback| {
                    callback(ev.code, ev.value);
                });
            }
        }
    });
}

fn init_mice() {
    let fd = std::fs::File::open("/dev/input/mice").expect("Failed to open input event device.");
    if fd.as_raw_fd() < 0 {
        return;
    }

    std::thread::spawn(move || {
        let mut buf = [0u8; 3];
        loop {
            std::thread::sleep(std::time::Duration::from_millis(8));
            let ret =
                unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, 3) };
            if ret < 0 {
                break;
            }
            MOUSE_CALLBACK.lock().unwrap().iter().for_each(|callback| {
                callback(buf);
            });
        }
    });
}

hal_fn_impl! {
    impl mod crate::hal_fn::dev::input {
        fn kbd_set_callback(callback: Box<dyn Fn(u16, i32) + Send + Sync>) {
            KBD_CALLBACK.lock().unwrap().push(callback);
        }

        fn mouse_set_callback(callback: Box<dyn Fn([u8; 3]) + Send + Sync>) {
            MOUSE_CALLBACK.lock().unwrap().push(callback);
        }

        fn init() {
            init_kbd();
            init_mice();
        }
    }
}
