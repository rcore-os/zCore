use crate::timer::now;
use core::time::Duration;

fn bench<F: FnMut()>(name: &str, n: u64, mut f: F) {
    let start = now();
    for _ in 0..n {
        f();
    }
    let end = now();
    println!("benchmark '{}' ({:?}): {:?}", name, Duration::from_nanos(end - start), Duration::from_nanos((end - start) / n));
}

pub fn benchmark_futex_wake() {
    bench("futex_wake", 500000, || {
        crate::futex::debug_wake_null();
    });
}

pub fn run_benchmarks() {
    benchmark_futex_wake();
}