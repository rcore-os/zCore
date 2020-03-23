pub fn init() {
    executor::spawn(async {
        info!("Hello! Async Rust!");
        // GG deadlock
        executor::spawn(async {
            info!("Nested task!");
        });
    });
}
