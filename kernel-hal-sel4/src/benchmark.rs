use crate::timer::now;
use core::time::Duration;

fn bench_custom<F: FnOnce(u64)>(name: &str, n: u64, f: F) {
    let start = now();
    f(n);
    let end = now();
    println!("benchmark '{}' ({:?}): {:?}", name, Duration::from_nanos(end - start), Duration::from_nanos((end - start) / n));
}

fn bench<F: FnMut()>(name: &str, n: u64, mut f: F) {
    bench_custom(name, n, move |n| {
        for _ in 0..n {
            f();
        } 
    });
}

pub fn benchmark_futex_wake() {
    bench("futex_wake", 500000, || {
        crate::futex::debug_wake_null();
    });
}

pub fn benchmark_vmalloc() {
    bench("vmalloc_two_pages", 20000, || {
        use crate::vm;
        vm::K.lock().allocate_region(0x100ff0000usize..0x100ff2000usize).unwrap();
        unsafe {
            assert_eq!(core::ptr::read_volatile(0x100ff1000usize as *mut u32), 0);
            core::ptr::write_volatile(0x100ff1000usize as *mut u32, 10);
            assert_eq!(core::ptr::read_volatile(0x100ff1000usize as *mut u32), 10);
        }
        vm::K.lock().release_region(0x100ff0000usize);
    });
}

pub fn benchmark_pmem_alloc() {
    bench("pmem_alloc", 50000, || {
        crate::pmem::Page::new().expect("benchmark_pmem_alloc: allocation failed");
    });
}

pub fn benchmark_kt_spawn() {
    bench_custom("kt_spawn", 2000, |n| {
        use crate::kt;
        use core::sync::atomic::{AtomicU64, Ordering};
        use alloc::sync::Arc;
        use crate::futex::FSem;
        let sem = Arc::new(FSem::new(0));
        let sem2 = sem.clone();

        kt::spawn(move || {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            for i in 0..n {
                let sem = sem2.clone();
                kt::spawn(move || {
                    if COUNTER.fetch_add(1, Ordering::Relaxed) + 1 == n {
                        sem.up();
                    }
                }).expect("benchmark_kt_spawn: spawn failed");
                crate::thread::yield_now();
            }
        }).expect("benchmark_kt_spawn: spawn failed");
        sem.down();
    });
}

pub fn benchmark_user_vm_fault() {
    use crate::user::{UserProcess};
    use trapframe::UserContext;
    let user_proc = UserProcess::new().expect("cannot create user process");
    let mut ut = Some(user_proc.create_thread().expect("cannot create user thread"));
    let mut uctx = UserContext::default();

    bench("user_vm_fault", 100000, || {
        let (entry_reason, next_ut) = ut.take().unwrap().run(&mut uctx);
        ut = Some(next_ut);
    })
}

pub fn benchmark_yield() {
    bench("yield", 1000000, || {
        crate::thread::yield_now();
    });
}

pub fn benchmark_timer_now() {
    bench("timer_now", 50000, || {
        crate::timer::now();
    });
}

pub fn run_benchmarks(rounds: u64) {
    for i in 0..rounds {
        println!("Round {}/{}", i + 1, rounds);
        benchmark_futex_wake();
        benchmark_vmalloc();
        benchmark_pmem_alloc();
        benchmark_yield();
        //benchmark_kt_spawn();
        benchmark_user_vm_fault();
        benchmark_timer_now();
    }
}