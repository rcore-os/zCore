//! Syscalls for time
//! - clock_gettime
//!
use crate::Syscall;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use core::convert::TryFrom;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use kernel_hal::{user::UserInPtr, user::UserOutPtr};
use lazy_static::lazy_static;
use linux_object::error::{LxError, SysResult};
use linux_object::process::ProcessExt;
use linux_object::signal::Signal;
use linux_object::thread::ThreadExt;
use linux_object::time::*;
use lock::Mutex;
use zircon_object::object::{KernelObject, KoID};
use zircon_object::task::{Thread, ROOT_JOB};

const USEC_PER_TICK: usize = 10000;

impl Syscall<'_> {
    /// finds the resolution (precision) of the specified clock clockid, and,
    /// if buffer is non-NULL, stores it in the struct timespec pointed to by buffer
    pub fn sys_clock_gettime(&self, clock: usize, mut buf: UserOutPtr<TimeSpec>) -> SysResult {
        info!("clock_gettime: id={:?} buf={:?}", clock, buf);
        if buf.is_null() {
            return Err(LxError::EINVAL);
        }
        let ts = match clock {
            0 | 5 => TimeSpec::now(), // CLOCK_REALTIME, CLOCK_REALTIME_COARSE
            1 | 4 | 6 | 7 => TimeSpec::now_monotonic(),
            _ => return Err(LxError::EINVAL),
        };
        buf.write(ts)?;

        info!("TimeSpec: {:?}", ts);

        Ok(0)
    }

    /// finds the resolution (precision) of the specified clock, and, if the
    /// buffer is non-NULL, stores it in the struct timespec pointed to by it.
    /// glibc/musl and some applications (e.g. Firefox) treat a garbage or
    /// unwritten resolution as a fatal condition, so always fill the struct.
    pub fn sys_clock_getres(&self, clock: usize, mut buf: UserOutPtr<TimeSpec>) -> SysResult {
        info!("clock_getres: id={:?} buf={:?}", clock, buf);
        // Reject unknown clocks the same way clock_gettime does.
        match clock {
            0..=7 => {}
            _ => return Err(LxError::EINVAL),
        }
        if buf.is_null() {
            return Ok(0);
        }
        // We service these clocks from a nanosecond-granularity timer source.
        let res = TimeSpec { sec: 0, nsec: 1 };
        buf.write(res)?;
        Ok(0)
    }

    /// set the time of the clock with id clockid
    pub fn sys_clock_settime(&self, clock: usize, timespec: UserInPtr<TimeSpec>) -> SysResult {
        info!(
            "clock_settime: id={:?} timespec={:?}",
            clock,
            timespec.read_if_not_null()?
        );
        if clock != 0 {
            return Err(LxError::EINVAL);
        }
        let ts = timespec.read()?;
        let target = Duration::new(ts.sec as u64, ts.nsec as u32);
        kernel_hal::timer::wall_clock_set(target);
        Ok(0)
    }

    /// legacy settimeofday (seconds + microseconds since Unix epoch)
    pub fn sys_settimeofday(&mut self, tv: UserInPtr<TimeVal>, tz: UserInPtr<u8>) -> SysResult {
        info!("settimeofday: tv={:?}, tz={:?}", tv, tz);
        if !tz.is_null() {
            return Err(LxError::EINVAL);
        }
        let timeval = tv.read()?;
        let target = Duration::new(timeval.sec as u64, timeval.usec as u32 * 1_000);
        kernel_hal::timer::wall_clock_set(target);
        Ok(0)
    }

    /// get the time with second and microseconds
    pub fn sys_gettimeofday(
        &mut self,
        mut tv: UserOutPtr<TimeVal>,
        tz: UserInPtr<u8>,
    ) -> SysResult {
        info!("gettimeofday: tv: {:?}, tz: {:?}", tv, tz);
        // don't support tz
        if !tz.is_null() {
            return Err(LxError::EINVAL);
        }

        let timeval = TimeVal::now();
        tv.write(timeval)?;

        info!("TimeVal: {:?}", timeval);

        Ok(0)
    }

    /// get time in seconds
    #[cfg(target_arch = "x86_64")]
    pub fn sys_time(&mut self, mut time: UserOutPtr<u64>) -> SysResult {
        info!("time: time: {:?}", time);
        if time.is_null() {
            return Err(LxError::EINVAL);
        }
        let sec = TimeSpec::now().sec;
        time.write(sec as u64)?;
        Ok(sec)
    }

    /// JUST FOR TEST, DO NOT USE IT
    pub fn sys_block_in_kernel(&self) -> SysResult {
        // DEAD LOOP
        error!("loop in kernel");
        let mut old = TimeSpec::now().sec;
        loop {
            let sec = TimeSpec::now().sec;
            if sec == old {
                core::hint::spin_loop();
                continue;
            }
            old = sec;
            warn!("1 seconds past");
        }
    }

    /// get resource usage
    /// (see [linux man getrusage(2)](https://www.man7.org/linux/man-pages/man2/getrusage.2.html)).
    ///
    /// `ru_utime` is the accumulated user-mode time of the target's threads
    /// (measured around every user-mode entry in the run loop); `ru_stime` is
    /// the process's accumulated in-kernel syscall time from the perf
    /// accounting. The previous implementation wrote the wall-clock
    /// time-since-boot into both fields, which made any "CPU used" computation
    /// (`time(1)`, build systems' self-profiling) nonsense.
    /// `RUSAGE_CHILDREN` reports zeros: usage of reaped children is not
    /// retained.
    pub fn sys_getrusage(&mut self, who: usize, mut rusage: UserOutPtr<RUsage>) -> SysResult {
        info!("getrusage: who: {}, rusage: {:?}", who, rusage);
        const RUSAGE_SELF: isize = 0;
        const RUSAGE_CHILDREN: isize = -1;
        const RUSAGE_THREAD: isize = 1;
        if rusage.is_null() {
            return Err(LxError::EINVAL);
        }
        let (utime_ns, stime_ns) = match who as isize {
            RUSAGE_SELF => (
                process_user_time_ns(self.zircon_process()),
                self.linux_process().perf().totals().1,
            ),
            // Per-thread kernel time is not split out of the process total;
            // report the thread's user time and zero kernel time.
            RUSAGE_THREAD => (self.thread.get_time(), 0),
            // Totals of children this process has reaped, accumulated at
            // wait4/waitid time exactly like Linux does.
            RUSAGE_CHILDREN => self.linux_process().children_cpu_ns(),
            _ => return Err(LxError::EINVAL),
        };
        rusage.write(RUsage {
            utime: Duration::from_nanos(utime_ns).into(),
            stime: Duration::from_nanos(stime_ns).into(),
            ..RUsage::default()
        })?;
        Ok(0)
    }

    /// stores the current process times in the struct tms that buf points to
    /// (see [linux man times(2)](https://www.man7.org/linux/man-pages/man2/times.2.html)).
    ///
    /// `tms_utime`/`tms_stime` come from the same accounting as
    /// [`sys_getrusage`](Self::sys_getrusage), converted to clock ticks
    /// (100 Hz here). Times of terminated children are not retained, so
    /// `tms_cutime`/`tms_cstime` read zero. The return value stays the
    /// wall-clock tick count since boot.
    pub fn sys_times(&mut self, mut buf: UserOutPtr<Tms>) -> SysResult {
        info!("times: buf: {:?}", buf);

        // 10_000 us per tick (100 Hz) → 10_000_000 ns per tick.
        const NSEC_PER_TICK: u64 = USEC_PER_TICK as u64 * 1_000;

        let tv = TimeVal::now();
        let tick = (tv.sec * 1_000_000 + tv.usec) / USEC_PER_TICK;

        if !buf.is_null() {
            let utime_ns = process_user_time_ns(self.zircon_process());
            let stime_ns = self.linux_process().perf().totals().1;
            let (cutime_ns, cstime_ns) = self.linux_process().children_cpu_ns();
            let new_buf = Tms {
                tms_utime: utime_ns / NSEC_PER_TICK,
                tms_stime: stime_ns / NSEC_PER_TICK,
                tms_cutime: cutime_ns / NSEC_PER_TICK,
                tms_cstime: cstime_ns / NSEC_PER_TICK,
            };
            buf.write(new_buf)?;
        } else {
            warn!("sys_times: Invalid buf {:x?}", buf);
        }

        info!("tick: {:?}", tick);
        Ok(tick)
    }

    /// clock nanosleep
    pub async fn sys_clock_nanosleep(
        &mut self,
        clockid: usize,
        flags: usize,
        req: UserInPtr<TimeSpec>,
        rem: UserOutPtr<TimeSpec>,
    ) -> SysResult {
        let _ = self.maybe_handle_tty_intr()?;
        info!(
            "clock_nanosleep: clockid={:?}, flags={:?}, req={:?}, rem={:?}",
            clockid,
            flags,
            req.read()?,
            rem
        );
        use core::time::Duration;
        use kernel_hal::{thread, timer};
        let duration: Duration = req.read()?.into();
        let clockid = ClockId::from(clockid);
        let flags = ClockFlags::from(flags);
        info!("clockid={:?}, flags={:?}", clockid, flags,);
        match clockid {
            ClockId::ClockRealTime => {
                match flags {
                    ClockFlags::ZeroFlag => {
                        thread::sleep_until(timer::deadline_after(duration)).await;
                    }
                    ClockFlags::TimerAbsTime => {
                        // 目前统一由nanosleep代替了、之后再修改
                        thread::sleep_until(timer::deadline_after(duration)).await;
                    }
                }
            }
            ClockId::ClockMonotonic => match flags {
                ClockFlags::ZeroFlag => {
                    thread::sleep_until(timer::deadline_after(duration)).await;
                }
                ClockFlags::TimerAbsTime => {
                    thread::sleep_until(timer::deadline_after(duration)).await;
                }
            },
            ClockId::ClockProcessCpuTimeId => {}
            ClockId::ClockThreadCpuTimeId => {}
            ClockId::ClockMonotonicRaw => {}
            ClockId::ClockRealTimeCoarse => {}
            ClockId::ClockMonotonicCoarse => {}
            ClockId::ClockBootTime => {}
            ClockId::ClockRealTimeAlarm => {}
            ClockId::ClockBootTimeAlarm => {}
        }
        Ok(0)
    }

    /// set value of an interval timer
    /// (see [linux man setitimer(2)](https://www.man7.org/linux/man-pages/man2/setitimer.2.html)).
    ///
    /// Full stateful semantics: the previous value (remaining time + interval)
    /// comes back through `old_value`, a non-zero `interval` re-arms the timer
    /// on every expiry, and writing a zero `value` disarms a pending timer —
    /// the piece the old fire-and-forget implementation missed, and what
    /// `alarm(0)` needs in order to actually cancel.
    ///
    /// Each timer kind delivers its own signal (SIGALRM / SIGVTALRM /
    /// SIGPROF). ITIMER_VIRTUAL and ITIMER_PROF should count process CPU
    /// time; this kernel does not account CPU time per process, so they tick
    /// on the wall clock — an upper bound of the correct expiry. Profilers get
    /// their SIGPROF stream, just at wall-clock rate.
    pub fn sys_setitimer(
        &mut self,
        which: usize,
        new_value: UserInPtr<ITimerVal>,
        mut old_value: UserOutPtr<ITimerVal>,
    ) -> SysResult {
        let val = new_value.read()?;
        info!(
            "setitimer: which={}, new_value={:?}, old_value={:?}",
            which, val, old_value
        );
        if which > ITIMER_PROF {
            return Err(LxError::EINVAL);
        }
        // Linux validates the microsecond fields' range.
        if val.value.usec >= 1_000_000 || val.interval.usec >= 1_000_000 {
            return Err(LxError::EINVAL);
        }
        let value = Duration::from(val.value);
        let interval = Duration::from(val.interval);
        let now = kernel_hal::timer::timer_now();
        let owner = self.zircon_process().id();
        let arm = {
            let mut slots = self.linux_process().itimers().lock();
            let slot = &mut slots[which];
            let old = itimerval_from_slot(slot, now);
            slot.generation += 1;
            let arm = if value.is_zero() {
                // Disarm. POSIX: a zero it_value stops the timer regardless of
                // what the interval field says.
                slot.interval = Duration::ZERO;
                slot.deadline = None;
                None
            } else {
                slot.interval = interval;
                let deadline = now + value;
                slot.deadline = Some(deadline);
                Some((deadline, slot.generation))
            };
            old_value.write_if_not_null(old)?;
            arm
        };
        if let Some((deadline, generation)) = arm {
            arm_itimer(owner, which, deadline, generation);
        }
        Ok(0)
    }

    /// get value of an interval timer
    /// (see [linux man getitimer(2)](https://www.man7.org/linux/man-pages/man2/getitimer.2.html)).
    ///
    /// Reports the live state kept by [`sys_setitimer`](Self::sys_setitimer):
    /// time remaining until the next expiry plus the reload interval.
    pub fn sys_getitimer(&self, which: usize, mut curr_value: UserOutPtr<ITimerVal>) -> SysResult {
        info!("getitimer: which={}", which);
        if which > ITIMER_PROF {
            return Err(LxError::EINVAL);
        }
        let now = kernel_hal::timer::timer_now();
        let val = {
            let slots = self.linux_process().itimers().lock();
            itimerval_from_slot(&slots[which], now)
        };
        curr_value.write(val)?;
        Ok(0)
    }

    /// Schedule SIGALRM (busybox `ping` uses this for read timeouts)
    /// (see [linux man alarm(2)](https://www.man7.org/linux/man-pages/man2/alarm.2.html)).
    ///
    /// Shares the ITIMER_REAL slot, exactly like Linux: `alarm` and
    /// `setitimer(ITIMER_REAL)` overwrite each other, `alarm(0)` cancels the
    /// pending alarm, and the return value is the seconds that were left on
    /// the previous one (rounded up, so a live timer never reports 0).
    pub fn sys_alarm(&self, seconds: usize) -> SysResult {
        info!("alarm: seconds={}", seconds);
        let now = kernel_hal::timer::timer_now();
        let owner = self.zircon_process().id();
        let (remaining_secs, arm) = {
            let mut slots = self.linux_process().itimers().lock();
            let slot = &mut slots[ITIMER_REAL];
            let remaining = slot
                .deadline
                .map(|d| d.saturating_sub(now))
                .unwrap_or_default();
            // Round up: returning 0 would mean "no alarm was pending".
            let remaining_secs =
                remaining.as_secs() as usize + usize::from(remaining.subsec_nanos() > 0);
            slot.generation += 1;
            slot.interval = Duration::ZERO;
            if seconds == 0 {
                slot.deadline = None;
                (remaining_secs, None)
            } else {
                let deadline = now + Duration::from_secs(seconds as u64);
                slot.deadline = Some(deadline);
                (remaining_secs, Some((deadline, slot.generation)))
            }
        };
        if let Some((deadline, generation)) = arm {
            arm_itimer(owner, ITIMER_REAL, deadline, generation);
        }
        Ok(remaining_secs)
    }

    /// `timer_create`: create a per-process POSIX interval timer. The notify
    /// signal comes from `sevp` (`struct sigevent`); a null `sevp` defaults to
    /// SIGALRM, `SIGEV_NONE` delivers no signal. The new timer id is written to
    /// `timerid` (an `int`, the kernel's `timer_t`).
    pub fn sys_timer_create(&self, _clockid: usize, sevp: usize, timerid: usize) -> SysResult {
        let signo = if sevp == 0 {
            Signal::SIGALRM as usize
        } else {
            // struct sigevent: sigev_value (8B), sigev_signo @ +8, sigev_notify @ +12.
            let signo_p: UserInPtr<i32> = (sevp + 8).into();
            let notify_p: UserInPtr<i32> = (sevp + 12).into();
            let signo = signo_p.read()?;
            let notify = notify_p.read()?;
            const SIGEV_NONE: i32 = 1;
            if notify == SIGEV_NONE {
                0
            } else {
                signo as usize
            }
        };
        let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed);
        POSIX_TIMERS.lock().insert(
            id,
            PosixTimer {
                owner: self.zircon_process().id(),
                signo,
                interval: Duration::ZERO,
                next: Duration::ZERO,
                generation: 0,
            },
        );
        let mut out: UserOutPtr<i32> = timerid.into();
        out.write(id as i32)?;
        Ok(0)
    }

    /// `timer_settime`: arm/disarm a timer. `it_value == 0` disarms; otherwise
    /// the timer fires at `it_value` (relative, or absolute with TIMER_ABSTIME)
    /// and then every `it_interval`.
    pub fn sys_timer_settime(
        &self,
        id: usize,
        flags: usize,
        new_value: UserInPtr<ITimerSpec>,
        mut old_value: UserOutPtr<ITimerSpec>,
    ) -> SysResult {
        const TIMER_ABSTIME: usize = 1;
        let owner = self.zircon_process().id();
        let spec = new_value.read()?;
        let interval = timespec_to_duration(spec.interval);
        let init = timespec_to_duration(spec.value);
        let now = kernel_hal::timer::timer_now();

        let (old, arm) = {
            let mut timers = POSIX_TIMERS.lock();
            let t = timers
                .get_mut(&id)
                .filter(|t| t.owner == owner)
                .ok_or(LxError::EINVAL)?;
            let remaining = t.next.checked_sub(now).unwrap_or(Duration::ZERO);
            let old = ITimerSpec {
                interval: TimeSpec::from_duration(t.interval),
                value: TimeSpec::from_duration(remaining),
            };
            // Bump the generation so any in-flight one-shot is dropped.
            t.generation += 1;
            t.interval = interval;
            let arm = if init.is_zero() {
                t.next = Duration::ZERO;
                None
            } else {
                let deadline = if flags & TIMER_ABSTIME != 0 {
                    init
                } else {
                    now + init
                };
                t.next = deadline;
                Some((deadline, t.generation))
            };
            (old, arm)
        };
        if !old_value.is_null() {
            old_value.write(old)?;
        }
        if let Some((deadline, gen)) = arm {
            arm_posix_timer(id, deadline, gen);
        }
        Ok(0)
    }

    /// `timer_gettime`: report the time until next expiration and the interval.
    pub fn sys_timer_gettime(&self, id: usize, curr_value: usize) -> SysResult {
        let owner = self.zircon_process().id();
        let now = kernel_hal::timer::timer_now();
        let out = {
            let timers = POSIX_TIMERS.lock();
            let t = timers
                .get(&id)
                .filter(|t| t.owner == owner)
                .ok_or(LxError::EINVAL)?;
            let remaining = if t.next.is_zero() {
                Duration::ZERO
            } else {
                t.next.checked_sub(now).unwrap_or(Duration::ZERO)
            };
            ITimerSpec {
                interval: TimeSpec::from_duration(t.interval),
                value: TimeSpec::from_duration(remaining),
            }
        };
        let mut p: UserOutPtr<ITimerSpec> = curr_value.into();
        p.write(out)?;
        Ok(0)
    }

    /// `timer_delete`: destroy a timer (cancels any pending fire via generation).
    pub fn sys_timer_delete(&self, id: usize) -> SysResult {
        let owner = self.zircon_process().id();
        let mut timers = POSIX_TIMERS.lock();
        match timers.get(&id) {
            Some(t) if t.owner == owner => {
                timers.remove(&id);
                Ok(0)
            }
            _ => Err(LxError::EINVAL),
        }
    }

    /// `timer_getoverrun`: we don't accumulate overruns, so report 0.
    pub fn sys_timer_getoverrun(&self, id: usize) -> SysResult {
        let owner = self.zircon_process().id();
        let timers = POSIX_TIMERS.lock();
        match timers.get(&id) {
            Some(t) if t.owner == owner => Ok(0),
            _ => Err(LxError::EINVAL),
        }
    }
}

/// A per-process POSIX interval timer (`timer_create`).
struct PosixTimer {
    /// Owning process KoID; a process may only operate on its own timers.
    owner: KoID,
    /// Signal to deliver on expiry (0 = none, e.g. SIGEV_NONE).
    signo: usize,
    /// Period for a periodic timer; `ZERO` = one-shot.
    interval: Duration,
    /// Absolute monotonic deadline of the next expiry; `ZERO` = disarmed.
    next: Duration,
    /// Bumped by settime/delete to invalidate an already-scheduled one-shot
    /// (`timer_set` callbacks are not cancellable, so they check this).
    generation: u64,
}

lazy_static! {
    static ref POSIX_TIMERS: Mutex<BTreeMap<usize, PosixTimer>> = Mutex::new(BTreeMap::new());
}
static NEXT_TIMER_ID: AtomicUsize = AtomicUsize::new(1);

fn timespec_to_duration(ts: TimeSpec) -> Duration {
    Duration::from_secs(ts.sec as u64) + Duration::from_nanos(ts.nsec as u64)
}

/// Accumulated user-mode nanoseconds of `proc`: its live threads plus the
/// process-level accumulator of already-exited ones (Linux keeps counting a
/// joined thread's time in the process totals). This is the utime side of
/// getrusage(2)/times(2); kernel-side time is accounted per-syscall by
/// `linux_object::perf` and reported as stime.
fn process_user_time_ns(proc: &alloc::sync::Arc<zircon_object::task::Process>) -> u64 {
    let live: u64 = proc
        .thread_ids()
        .into_iter()
        .filter_map(|tid| proc.get_child(tid).ok())
        .filter_map(|obj| obj.downcast_arc::<Thread>().ok())
        .map(|t| t.get_time())
        .sum();
    live + proc.dead_threads_time()
}

/// `ITIMER_*` indices from the uapi.
const ITIMER_REAL: usize = 0;
const ITIMER_VIRTUAL: usize = 1;
const ITIMER_PROF: usize = 2;

/// The signal an expiring interval-timer slot delivers (setitimer(2)).
fn itimer_signo(which: usize) -> usize {
    match which {
        ITIMER_VIRTUAL => Signal::SIGVTALRM as usize,
        ITIMER_PROF => Signal::SIGPROF as usize,
        _ => Signal::SIGALRM as usize,
    }
}

/// Render a slot as the userspace `struct itimerval`: the reload interval plus
/// the time remaining until expiry (all zeros when disarmed). Pure, so the
/// remaining-time arithmetic is unit-testable.
fn itimerval_from_slot(slot: &ItimerSlot, now: Duration) -> ITimerVal {
    ITimerVal {
        interval: slot.interval.into(),
        value: slot
            .deadline
            .map(|d| d.saturating_sub(now))
            .unwrap_or_default()
            .into(),
    }
}

/// Schedule the one-shot that fires itimer `which` of process `owner` at
/// `deadline`, valid only while the slot's generation still matches `gen`
/// (the same protocol as [`arm_posix_timer`]). A periodic timer re-arms
/// itself from the callback; the process is looked up by id each time, so an
/// exited process just fails the lookup and the chain stops without the timer
/// wheel keeping the process object alive.
fn arm_itimer(owner: KoID, which: usize, deadline: Duration, gen: u64) {
    kernel_hal::timer::timer_set(
        deadline,
        Box::new(move |_now| {
            let mut fire = false;
            let mut rearm = None;
            if let Some(proc) = ROOT_JOB.find_process(owner) {
                if let Some(lp) = proc.try_linux() {
                    let mut slots = lp.itimers().lock();
                    let slot = &mut slots[which];
                    if slot.generation == gen {
                        if let Some(expiry) = slot.deadline {
                            fire = true;
                            if slot.interval.is_zero() {
                                slot.deadline = None;
                            } else {
                                // Drift-free: step from the programmed expiry,
                                // not from whenever the wheel got to us.
                                let next = expiry + slot.interval;
                                slot.deadline = Some(next);
                                rearm = Some(next);
                            }
                        }
                    }
                }
            }
            if fire {
                deliver_timer_signal(owner, itimer_signo(which));
            }
            if let Some(next) = rearm {
                arm_itimer(owner, which, next, gen);
            }
        }),
    );
}

/// Deliver `signo` to every thread of process `owner` (mirrors setitimer/alarm).
fn deliver_timer_signal(owner: KoID, signo: usize) {
    if signo == 0 {
        return;
    }
    let signal = match Signal::try_from(signo as u8) {
        Ok(s) => s,
        Err(_) => return,
    };
    if let Some(proc) = ROOT_JOB.find_process(owner) {
        for tid in proc.thread_ids() {
            if let Ok(obj) = proc.get_child(tid) {
                if let Ok(thread) = obj.downcast_arc::<Thread>() {
                    thread.lock_linux().signals.insert(signal);
                    thread.signal_set(zircon_object::object::Signal::USER_SIGNAL_0);
                }
            }
        }
    }
}

/// Schedule the one-shot that fires timer `id` at `deadline`, valid only while
/// the timer still exists and its generation matches `gen`. A periodic timer
/// re-arms itself from the callback.
fn arm_posix_timer(id: usize, deadline: Duration, gen: u64) {
    kernel_hal::timer::timer_set(
        deadline,
        Box::new(move |_now| {
            let mut fire = None;
            let mut rearm = None;
            {
                let mut timers = POSIX_TIMERS.lock();
                if let Some(t) = timers.get_mut(&id) {
                    if t.generation == gen {
                        fire = Some((t.owner, t.signo));
                        if t.interval.is_zero() {
                            t.next = Duration::ZERO;
                        } else {
                            t.next += t.interval;
                            rearm = Some(t.next);
                        }
                    }
                }
            }
            if let Some((owner, signo)) = fire {
                deliver_timer_signal(owner, signo);
            }
            if let Some(deadline) = rearm {
                arm_posix_timer(id, deadline, gen);
            }
        }),
    );
}

#[cfg(test)]
mod itimer_tests {
    use super::*;

    #[test]
    fn disarmed_slot_reads_all_zeros() {
        let slot = ItimerSlot::default();
        let v = itimerval_from_slot(&slot, Duration::from_secs(100));
        assert_eq!((v.value.sec, v.value.usec), (0, 0));
        assert_eq!((v.interval.sec, v.interval.usec), (0, 0));
    }

    #[test]
    fn armed_slot_reports_remaining_and_interval() {
        let slot = ItimerSlot {
            interval: Duration::from_millis(1500),
            deadline: Some(Duration::from_secs(10)),
            generation: 3,
        };
        let v = itimerval_from_slot(&slot, Duration::from_millis(7750));
        // 10s - 7.75s = 2.25s remaining.
        assert_eq!((v.value.sec, v.value.usec), (2, 250_000));
        assert_eq!((v.interval.sec, v.interval.usec), (1, 500_000));
    }

    #[test]
    fn expired_deadline_saturates_to_zero() {
        let slot = ItimerSlot {
            interval: Duration::ZERO,
            deadline: Some(Duration::from_secs(5)),
            generation: 1,
        };
        let v = itimerval_from_slot(&slot, Duration::from_secs(9));
        assert_eq!((v.value.sec, v.value.usec), (0, 0));
    }

    #[test]
    fn itimer_slots_deliver_their_own_signals() {
        assert_eq!(itimer_signo(ITIMER_REAL), Signal::SIGALRM as usize);
        assert_eq!(itimer_signo(ITIMER_VIRTUAL), Signal::SIGVTALRM as usize);
        assert_eq!(itimer_signo(ITIMER_PROF), Signal::SIGPROF as usize);
    }
}
