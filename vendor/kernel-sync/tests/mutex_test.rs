// extern crate alloc;
// use alloc::sync::Arc;
// use alloc::vec;
// use kernel_sync::mutex::Mutex;

// use tokio;
// async fn handle(x: Arc<Mutex<i32>>, loop_cnt: i32) {
//     for _ in 0..loop_cnt {
//         let mut guard = x.lock().await;
//         *guard += 1;
//     }
// }

// #[tokio::test]
// async fn mutex_test() {
//     let x = Arc::new(Mutex::new(0));
//     let coroutine_cnt = 10;
//     let loop_cnt = 500;
//     let mut coroutines = vec![];
//     for _ in 0..coroutine_cnt {
//         let x_cloned = x.clone();
//         coroutines.push(tokio::spawn(handle(x_cloned, loop_cnt)));
//     }
//     for coroutine in coroutines {
//         tokio::join!(coroutine).0.unwrap();
//     }
//     assert_eq!(*x.lock().await, coroutine_cnt * loop_cnt);
// }
