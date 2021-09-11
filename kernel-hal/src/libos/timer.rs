use std::time::{Duration, SystemTime};

hal_fn_impl! {
    impl mod crate::hal_fn::timer {
        fn timer_now() -> Duration {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
        }

        fn timer_set(deadline: Duration, callback: Box<dyn FnOnce(Duration) + Send + Sync>) {
            std::thread::spawn(move || {
                let now = timer_now();
                if deadline > now {
                    std::thread::sleep(deadline - now);
                }
                callback(timer_now());
            });
        }
    }
}
