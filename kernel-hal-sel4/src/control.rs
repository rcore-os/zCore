use crate::types::*;
use crate::error::*;
use crate::kipc::{KipcChannel, SavedReplyHandle};
use lazy_static::lazy_static;
use crate::kt::{spawn, KernelThread};
use alloc::boxed::Box;
use crate::timer;
use crate::futex::FMutex;
use alloc::collections::btree_map::BTreeMap;
use alloc::collections::linked_list::LinkedList;

const BUSY_TIMER_THRESHOLD: u64 = 1000000 * 55; // 55ms
const BUSY_TIMER_PERIOD: u64 = 1000000; // 1ms
const IDLE_TIMER_PERIOD: u64 = 1000000 * 50; // 50ms

lazy_static! {
    static ref CONTROL: KipcChannel<ControlMessage> = KipcChannel::new().expect("kipc/CONTROL: init failed");
}

static SLEEP_QUEUE: FMutex<SleepQueue> = FMutex::new(SleepQueue::new());

struct SleepQueue {
    queue: BTreeMap<u64, LinkedList<SavedReplyHandle>>,
    busy_mode: bool,
}

impl SleepQueue {
    const fn new() -> SleepQueue {
        SleepQueue {
            queue: BTreeMap::new(),
            busy_mode: false,
        }
    }

    fn update_busy_mode(&mut self, now: u64) {
        let first_deadline = self.queue.first_key_value().map(|x| *x.0);
        let mut should_enable = false;

        if let Some(x) = first_deadline {
            if x < now || x - now < BUSY_TIMER_THRESHOLD {
                should_enable = true;
            }
        }

        if should_enable && !self.busy_mode {
            self.busy_mode = true;
            unsafe {
                timer::set_period(BUSY_TIMER_PERIOD).expect("failed to set timer period");
            }
        }

        if !should_enable && self.busy_mode {
            self.busy_mode = false;
            unsafe {
                timer::set_period(IDLE_TIMER_PERIOD).expect("failed to set timer period");
            }
        }
    }
}

enum ControlMessage {
    ExitThread(KernelThread),
    SleepNs(u64),
    IdleForever,
}

fn run() -> ! {
    // Init timer
    unsafe {
        timer::set_period(IDLE_TIMER_PERIOD).expect("failed to set initial timer period");
    }
    spawn(|| {
        kt_timerd();
    }).expect("failed to spawn timerd");

    loop {
        let (msg, reply) = CONTROL.recv();
        //println!("Got control message");
        match msg {
            ControlMessage::ExitThread(t) => {
                unsafe {
                    t.drop_from_control_thread();
                }

                // No need to reply to an exited thread
                reply.forget();
            }
            ControlMessage::SleepNs(n) => {
                // FIXME: OOM kill
                let handle = reply.save().expect("cannot save reply");
                let now = timer::now();
                let deadline = now.saturating_add(n);

                let mut sleep_queue = SLEEP_QUEUE.lock();
                sleep_queue.queue.entry(deadline).or_insert(LinkedList::new()).push_back(handle);
                sleep_queue.update_busy_mode(now);
            }
            ControlMessage::IdleForever => {
                reply.forget();
            }
        }
    }
}

pub fn init() {
    spawn(|| {
        run();
    }).expect("control::init: cannot spawn thread");
}

pub fn exit_thread(kt: KernelThread) -> ! {
    drop(CONTROL.call(ControlMessage::ExitThread(kt)));
    unreachable!("exit_thread");
}

pub fn sleep(ns: u64) {
    CONTROL.call(ControlMessage::SleepNs(ns)).expect("sleep: control call failed");
}

pub fn idle() -> ! {
    drop(CONTROL.call(ControlMessage::IdleForever));
    unreachable!("idle");
}

fn kt_timerd() -> ! {
    loop {
        let now = unsafe {
            timer::wait()
        };
        let mut sleep_queue = SLEEP_QUEUE.lock();
        while let Some(entry) = sleep_queue.queue.first_entry() {
            if *entry.key() <= now {
                let reply_set = entry.remove();
                for reply in reply_set {
                    reply.send(0)
                }
            } else {
                // We finished the wake-up process.
                break;
            }
        }
        sleep_queue.update_busy_mode(now);
    }
}