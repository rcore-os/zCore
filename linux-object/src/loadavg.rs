//! Process accounting and approximate, Linux-style system load averages.
//!
//! Linux keeps three exponentially-weighted moving averages of the
//! run-queue length, sampled every 5 seconds (the 1-, 5- and 15-minute
//! windows) from the scheduler tick — crucially, at instants *uncorrelated*
//! with whoever reads `/proc/loadavg`. [`sampler_task`] reproduces that: a
//! kernel task advances the averages every 5 s from the executor's global
//! run-queue length.
//!
//! The previous design advanced the averages lazily at read time using the
//! count of threads executing at that instant. That correlates the sample
//! with the reader's own wake-up: a status bar that polls `/proc/loadavg`
//! whenever the desktop redraws always finds exactly the compositor mid-poll
//! next to it, so an idle desktop reported a rock-steady load of 1.00. The
//! lazy path is kept only as a fallback for hosted (libos) builds, where no
//! sampler task is spawned and the executor run queue is not visible.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use kernel_hal::timer::timer_now;
use lazy_static::lazy_static;
use lock::Mutex;
use zircon_object::object::KernelObject;
use zircon_object::task::{running_thread_count, Job, Process, Status, ROOT_JOB};

/// Fixed-point shift used by the EWMA math (matches Linux `FSHIFT`).
const FSHIFT: u32 = 11;
/// 1.0 in the internal fixed-point format.
const FIXED_1: u64 = 1 << FSHIFT;
/// Shift used by `sysinfo(2)`'s `loads` field (`SI_LOAD_SHIFT`).
const SI_LOAD_SHIFT: u32 = 16;
/// Sampling period, in seconds (Linux samples every 5s).
const LOAD_FREQ_SECS: u64 = 5;
/// Decay factors for the 1/5/15-minute windows at a 5s sample period —
/// `exp(-5/60)`, `exp(-5/300)`, `exp(-5/900)` in `FSHIFT` fixed point.
const EXP_1: u64 = 1884;
const EXP_5: u64 = 2014;
const EXP_15: u64 = 2037;
/// Cap on catch-up iterations so a long gap between reads can't spin: 256
/// steps is 21 min of samples, well past the 15-minute window's settling.
const MAX_CATCHUP_STEPS: u32 = 256;

struct LoadAvg {
    /// Seconds-since-boot at which the averages were last advanced (0 = never).
    last_secs: u64,
    /// The three averages, in `FSHIFT` fixed point.
    loads: [u64; 3],
}

lazy_static! {
    static ref STATE: Mutex<LoadAvg> = Mutex::new(LoadAvg {
        last_secs: 0,
        loads: [0; 3],
    });
}

fn collect(job: &Arc<Job>, out: &mut Vec<Arc<Process>>) {
    for id in job.process_ids() {
        if let Some(proc) = job.find_process(id) {
            if !matches!(proc.status(), Status::Exited(_)) {
                out.push(proc);
            }
        }
    }
    for child_id in job.children_ids() {
        if let Ok(child) = job.get_child(child_id) {
            if let Ok(child_job) = child.downcast_arc::<Job>() {
                collect(&child_job, out);
            }
        }
    }
}

/// Count of `(live processes, currently-runnable threads)` across all jobs.
///
/// "Live" is every process that has started and not yet exited. "Runnable" is
/// *not* the same as "live": a process is alive for its whole lifetime even
/// while every one of its threads sits blocked in a syscall, so counting live
/// processes makes an idle box report a load average equal to its process
/// count. Instead we ask the executor how many threads are actually executing
/// right now ([`running_thread_count`]) and subtract one for the sampler thread
/// itself (the caller is, by definition, running while it reads this).
pub fn count_processes() -> (usize, usize) {
    let mut procs = Vec::new();
    collect(&ROOT_JOB, &mut procs);
    let total = procs.len();
    let running = runnable_count();
    (total, running)
}

/// Number of runnable tasks system-wide, excluding the caller.
///
/// Prefers the executor's run-queue length (queued + currently-polled tasks,
/// which includes the calling thread — hence the `- 1`): unlike counting only
/// threads mid-poll, it also sees runnable-but-preempted work. On a fully idle
/// system only the caller occupies the queue — giving 0, matching a real Linux
/// idle box. Hosted (libos) builds report a queue length of 0 and fall back to
/// the old executing-thread count.
pub fn runnable_count() -> usize {
    let queued = kernel_hal::thread::runnable_task_count();
    if queued > 0 {
        queued - 1
    } else {
        running_thread_count().saturating_sub(1)
    }
}

/// Linux's `calc_load`: decay `load` toward `active` by factor `exp`.
fn calc_load(load: u64, exp: u64, active: u64) -> u64 {
    let mut newload = load * exp + active * (FIXED_1 - exp);
    if active >= load {
        newload += FIXED_1 - 1;
    }
    newload / FIXED_1
}

/// Set once the periodic sampler has taken its first sample. From then on
/// reads only report state — the lazy read-time advance (which correlates the
/// sample with the reader and biases the average) stays off for good.
static SAMPLER_LIVE: AtomicBool = AtomicBool::new(false);

/// Advance the EWMA windows up to `now`, decaying toward `active` (the
/// runnable count in `FSHIFT` fixed point) for every elapsed 5 s step.
fn advance(active: u64) {
    let now = timer_now().as_secs();
    let mut g = STATE.lock();
    if g.last_secs == 0 {
        g.last_secs = now;
    }
    let mut steps = 0;
    while now >= g.last_secs + LOAD_FREQ_SECS && steps < MAX_CATCHUP_STEPS {
        g.last_secs += LOAD_FREQ_SECS;
        let l = g.loads;
        g.loads[0] = calc_load(l[0], EXP_1, active);
        g.loads[1] = calc_load(l[1], EXP_5, active);
        g.loads[2] = calc_load(l[2], EXP_15, active);
        steps += 1;
    }
    // Skipped a large gap (e.g. the sampler was started late): snap the clock
    // forward so the next advance doesn't re-walk the whole interval.
    if steps == MAX_CATCHUP_STEPS {
        g.last_secs = now;
    }
}

/// Return the averages in `FSHIFT` fixed point. With the periodic sampler
/// live, this is a pure read; otherwise (hosted builds) it advances lazily
/// from the caller's instantaneous view, as before.
fn sample() -> [u64; 3] {
    if !SAMPLER_LIVE.load(Ordering::Relaxed) {
        let (_total, running) = count_processes();
        advance((running as u64) * FIXED_1);
    }
    STATE.lock().loads
}

/// Kernel-side load sampler: advances the averages every 5 s from the
/// executor's global run-queue length, uncorrelated with any reader. Spawned
/// once at boot (see the loader's deferred boot work); never returns.
pub async fn sampler_task() {
    loop {
        kernel_hal::thread::sleep_until(timer_now() + Duration::from_secs(LOAD_FREQ_SECS)).await;
        // This task occupies one run-queue slot while it samples; subtract it
        // so a fully idle box reads 0.00.
        let running = kernel_hal::thread::runnable_task_count().saturating_sub(1);
        advance((running as u64) * FIXED_1);
        SAMPLER_LIVE.store(true, Ordering::Relaxed);
    }
}

/// Load averages as `f64` (1, 5, 15 minutes), for textual reports like
/// `/proc/loadavg`.
pub fn loadavg_f64() -> [f64; 3] {
    let l = sample();
    [
        l[0] as f64 / FIXED_1 as f64,
        l[1] as f64 / FIXED_1 as f64,
        l[2] as f64 / FIXED_1 as f64,
    ]
}

/// Load averages in the fixed point `sysinfo(2)` expects (`<< SI_LOAD_SHIFT`).
pub fn loadavg_sysinfo() -> [u64; 3] {
    let l = sample();
    let shift = SI_LOAD_SHIFT - FSHIFT;
    [l[0] << shift, l[1] << shift, l[2] << shift]
}
