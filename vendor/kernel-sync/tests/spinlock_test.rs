extern crate alloc;
use alloc::sync::Arc;
use alloc::vec;
use kernel_sync::mutex::Mutex;

#[test]
fn basic_test() {
    let x = Arc::new(SpinLock::new(0));
    let thread_cnt = 3;
    let loop_cnt = 1000000;
    let mut threads = vec![];
    for _ in 0..thread_cnt {
        let x_clone = x.clone();
        threads.push(std::thread::spawn(move || {
            for _ in 0..loop_cnt {
                let mut guard = x_clone.lock();
                *guard += 1;
            }
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(*(x.lock()), thread_cnt * loop_cnt);
}

#[test]
fn try_lock_test() {
    let x = Arc::new(SpinLock::new(0));
    let lock_result0 = x.try_lock();
    assert!(lock_result0.is_some());

    let lock_result1 = x.try_lock();
    assert!(lock_result1.is_none());

    drop(lock_result0);

    let lock_result2 = x.try_lock();
    assert!(lock_result2.is_some());
}
