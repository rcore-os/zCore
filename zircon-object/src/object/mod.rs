//! Kernel object basis.
//!
//! # Create new kernel object
//!
//! - Create a new struct.
//! - Make sure it has a field named `base` with type [`KObjectBase`].
//! - Implement [`KernelObject`] trait with [`impl_kobject`] macro.
//!
//! ## Example
//! ```
//! use zircon_object::object::*;
//! extern crate alloc;
//!
//! pub struct SampleObject {
//!    base: KObjectBase,
//! }
//! impl_kobject!(SampleObject);
//! ```
//!
//! # Implement methods for kernel object
//!
//! ## Constructor
//!
//! Each kernel object should have a constructor returns `Arc<Self>`
//! (or a pair of them, e.g. [`Channel`]).
//!
//! Don't return `Self` since it must be created on heap.
//!
//! ### Example
//! ```
//! use zircon_object::object::*;
//! use std::sync::Arc;
//!
//! pub struct SampleObject {
//!     base: KObjectBase,
//! }
//! impl SampleObject {
//!     pub fn new() -> Arc<Self> {
//!         Arc::new(SampleObject {
//!             base: KObjectBase::new(),
//!         })
//!     }
//! }
//! ```
//!
//! ## Interior mutability
//!
//! All kernel objects use the [interior mutability pattern] :
//! each method takes either `&self` or `&Arc<Self>` as the first argument.
//!
//! To handle mutable variable, create another **inner structure**,
//! and put it into the object with a lock wrapped.
//!
//! ### Example
//! ```
//! use zircon_object::object::*;
//! use std::sync::Arc;
//! use lock::Mutex;
//!
//! pub struct SampleObject {
//!     base: KObjectBase,
//!     inner: Mutex<SampleObjectInner>,
//! }
//! struct SampleObjectInner {
//!     x: usize,
//! }
//!
//! impl SampleObject {
//!     pub fn set_x(&self, x: usize) {
//!         let mut inner = self.inner.lock();
//!         inner.x = x;
//!     }
//! }
//! ```
//!
//! # Downcast trait to concrete type
//!
//! [`KernelObject`] inherit [`downcast_rs::DowncastSync`] trait.
//! You can use `downcast_arc` method to downcast `Arc<dyn KernelObject>` to `Arc<T: KernelObject>`.
//!
//! ## Example
//! ```
//! use zircon_object::object::*;
//! use std::sync::Arc;
//!
//! let object: Arc<dyn KernelObject> = DummyObject::new();
//! let concrete = object.downcast_arc::<DummyObject>().unwrap();
//! ```
//!
//! [`Channel`]: crate::ipc::Channel
//! [`KObjectBase`]: KObjectBase
//! [`KernelObject`]: KernelObject
//! [`impl_kobject`]: impl_kobject
//! [`downcast_rs::DowncastSync`]: downcast_rs::DowncastSync
//! [interior mutability pattern]: https://doc.rust-lang.org/reference/interior-mutability.html

use {
    crate::signal::*,
    alloc::{boxed::Box, string::String, sync::Arc, vec::Vec},
    core::{
        fmt::Debug,
        future::Future,
        pin::Pin,
        sync::atomic::*,
        task::{Context, Poll},
    },
    downcast_rs::{impl_downcast, DowncastSync},
    lock::Mutex,
};

pub use {super::*, handle::*, rights::*, signal::*};

mod handle;
mod rights;
pub mod signal;

/// Common interface of a kernel object.
///
/// Implemented by [`impl_kobject`] macro.
///
/// [`impl_kobject`]: impl_kobject
pub trait KernelObject: DowncastSync + Debug {
    /// Get object's KoID.
    fn id(&self) -> KoID;
    /// Get the name of the type of the kernel object.
    fn type_name(&self) -> &str;
    /// Get object's name.
    fn name(&self) -> alloc::string::String;
    /// Set object's name.
    fn set_name(&self, name: &str);
    /// Get the signal status.
    fn signal(&self) -> Signal;
    /// Assert `signal`.
    fn signal_set(&self, signal: Signal);
    /// Deassert `signal`.
    fn signal_clear(&self, signal: Signal);
    /// Change signal status: first `clear` then `set` indicated bits.
    ///
    /// All signal callbacks will be called.
    fn signal_change(&self, clear: Signal, set: Signal);
    /// Add `callback` for signal status changes.
    ///
    /// The `callback` is a function of `Fn(Signal) -> bool`.
    /// It returns a bool indicating whether the handle process is over.
    /// If true, the function will never be called again.
    fn add_signal_callback(&self, callback: SignalHandler);
    /// Attempt to find a child of the object with given KoID.
    ///
    /// If the object is a *Process*, the *Threads* it contains may be obtained.
    ///
    /// If the object is a *Job*, its (immediate) child *Jobs* and the *Processes*
    /// it contains may be obtained.
    ///
    /// If the object is a *Resource*, its (immediate) child *Resources* may be obtained.
    fn get_child(&self, _id: KoID) -> ZxResult<Arc<dyn KernelObject>> {
        Err(ZxError::WRONG_TYPE)
    }
    /// Attempt to get the object's peer.
    ///
    /// An object peer is the opposite endpoint of a `Channel`, `Socket`, `Fifo`, or `EventPair`.
    fn peer(&self) -> ZxResult<Arc<dyn KernelObject>> {
        Err(ZxError::NOT_SUPPORTED)
    }
    /// If the object is related to another (such as the other end of a channel, or the parent of
    /// a job), returns the KoID of that object, otherwise returns zero.
    fn related_koid(&self) -> KoID {
        0
    }
    /// Get object's allowed signals.
    fn allowed_signals(&self) -> Signal {
        Signal::USER_ALL
    }
}

impl_downcast!(sync KernelObject);

/// The base struct of a kernel object.
pub struct KObjectBase {
    /// The object's KoID.
    pub id: KoID,
    /// Whether `id` was drawn from the recycling PID pool (processes/threads
    /// only) and must be returned to it on drop. `with_id` and plain-object
    /// constructors never set this, so fixed ids (init=1, shells 101..) and
    /// high monotonic ids are never fed back into the pool.
    pooled: bool,
    inner: Mutex<KObjectBaseInner>,
}

/// Recycling allocator for the ids userspace sees as Linux pids/tids.
///
/// Only `Process` and `Thread` draw from here (via
/// [`KObjectBase::with_name_pooled`]); every other kernel object takes a
/// monotonic id ≥ 2^32 from [`KObjectBase::new_koid`], so object churn (VMOs,
/// handles, channels — hundreds per spawn) no longer inflates the pid space.
/// Ids live in `[FLOOR, CEIL)`; `CEIL` matches the `pid_max` procfs announces,
/// so a pid is always a valid Linux `pid_t` and never crosses the i32 border
/// no matter the uptime.
///
/// An id returns to the pool when its `KObjectBase` drops — and a parent holds
/// an `Arc` of each child until it is reaped, so a pid cannot be reused while
/// a zombie still wears it: Linux's rule, enforced by lifetime instead of
/// bookkeeping.
mod pid_pool {
    use alloc::collections::VecDeque;
    use core::sync::atomic::{AtomicU64, Ordering};
    use lock::Mutex;

    /// First pooled pid. Everything below is reserved for fixed ids
    /// (`with_id`: init=1, per-tty shells 101..106).
    pub(super) const FLOOR: u64 = 1024;
    /// One past the highest pooled pid; equals Linux's PID_MAX_LIMIT and the
    /// value `/proc/sys/kernel/pid_max` reports.
    pub(super) const CEIL: u64 = 4_194_304;

    struct Pool {
        free: VecDeque<u32>,
        next_fresh: u32,
    }

    static POOL: Mutex<Pool> = Mutex::new(Pool {
        free: VecDeque::new(),
        next_fresh: FLOOR as u32,
    });
    static RECYCLED: AtomicU64 = AtomicU64::new(0);

    /// A pooled pid, or `None` when over 4M processes/threads are alive at
    /// once (the caller falls back to a high monotonic id rather than fail).
    pub(super) fn alloc() -> Option<u64> {
        let mut p = POOL.lock();
        if let Some(id) = p.free.pop_front() {
            let n = RECYCLED.fetch_add(1, Ordering::Relaxed);
            if n == 0 {
                // One line per boot: proof in the log that recycling is live.
                log::info!("[pid] first recycled pid: {}", id);
            }
            return Some(id as u64);
        }
        if (p.next_fresh as u64) < CEIL {
            let id = p.next_fresh;
            p.next_fresh += 1;
            return Some(id as u64);
        }
        None
    }

    pub(super) fn release(id: u64) {
        debug_assert!((FLOOR..CEIL).contains(&id));
        static RELEASED: AtomicU64 = AtomicU64::new(0);
        let n = RELEASED.fetch_add(1, Ordering::Relaxed);
        if n == 0 {
            // One line per boot: the release side of the pool is alive too —
            // its absence under churn means task objects are being leaked
            // (an Arc holder outliving reap), which is a bug to chase, not a
            // pool problem.
            log::info!("[pid] first pid released: {}", id);
        }
        POOL.lock().free.push_back(id as u32);
    }
}

impl Drop for KObjectBase {
    fn drop(&mut self) {
        if self.pooled {
            pid_pool::release(self.id);
        }
    }
}

const MAX_SIGNAL_CALLBACKS: usize = 1024;

/// The mutable part of `KObjectBase`.
#[derive(Default)]
struct KObjectBaseInner {
    name: String,
    signal: Signal,
    signal_callbacks: Vec<SignalHandler>,
}

impl Default for KObjectBase {
    fn default() -> Self {
        KObjectBase {
            id: Self::new_koid(),
            pooled: false,
            inner: Default::default(),
        }
    }
}

impl KObjectBase {
    /// Create a new kernel object base.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a kernel object base with initial `signal`.
    pub fn with_signal(signal: Signal) -> Self {
        KObjectBase::with(Default::default(), signal)
    }

    /// Create a kernel object base with `name`.
    pub fn with_name(name: &str) -> Self {
        KObjectBase::with(name, Default::default())
    }

    /// Create a kernel object base with both signal and name
    pub fn with(name: &str, signal: Signal) -> Self {
        let base = KObjectBase {
            id: Self::new_koid(),
            pooled: false,
            inner: Mutex::new(KObjectBaseInner {
                name: String::from(name),
                signal,
                ..Default::default()
            }),
        };
        watch_name_if_requested(&base.inner.lock().name);
        base
    }

    /// Create a kernel object with a fixed KoID (e.g. Linux PID 1 for init).
    pub fn with_id(id: KoID, name: &str, signal: Signal) -> Self {
        KObjectBase {
            id,
            pooled: false,
            inner: Mutex::new(KObjectBaseInner {
                name: String::from(name),
                signal,
                ..Default::default()
            }),
        }
    }

    /// Create a kernel object base whose id is a RECYCLING Linux pid/tid.
    ///
    /// Only `Process` and `Thread` use this: their ids are the pids/tids
    /// userspace sees, so they must stay small (< `pid_max`) and be reused
    /// after death — Linux semantics. Every other object keeps a monotonic
    /// id ≥ 2^32 that can never collide with a pid.
    pub fn with_name_pooled(name: &str) -> Self {
        if let Some(id) = pid_pool::alloc() {
            let base = KObjectBase {
                id,
                pooled: true,
                inner: Mutex::new(KObjectBaseInner {
                    name: String::from(name),
                    ..Default::default()
                }),
            };
            watch_name_if_requested(&base.inner.lock().name);
            return base;
        }
        // Over 4M live processes/threads — practically unreachable, but a
        // process with a big (non-pid_t) id beats failing the spawn.
        log::error!("[pid] pool exhausted (>4M live tasks); falling back to a high monotonic id");
        Self::with_name(name)
    }

    /// Generate a new KoID.
    ///
    /// Starts at 2^32: ids of plain kernel objects (VMOs, handles, channels,
    /// timers — allocated by the hundreds per process spawn) live strictly
    /// above the pid space, so object churn can never push a value that
    /// userspace treats as a `pid_t` past any limit. At 2^32..2^64 there is
    /// headroom for millions of years of allocation.
    fn new_koid() -> KoID {
        static KOID: AtomicU64 = AtomicU64::new(1 << 32);
        KOID.fetch_add(1, Ordering::SeqCst)
    }

    /// Get object's name.
    pub fn name(&self) -> String {
        self.inner.lock().name.clone()
    }

    /// Set object's name.
    pub fn set_name(&self, name: &str) {
        let mut inner = self.inner.lock();
        inner.name = String::from(name);
        watch_name_if_requested(&inner.name);
    }

    /// Get the signal status.
    pub fn signal(&self) -> Signal {
        self.inner.lock().signal
    }

    /// Change signal status: first `clear` then `set` indicated bits.
    ///
    /// All signal callbacks will be called.
    pub fn signal_change(&self, clear: Signal, set: Signal) {
        let mut inner = self.inner.lock();
        let old_signal = inner.signal;
        inner.signal.remove(clear);
        inner.signal.insert(set);
        let new_signal = inner.signal;
        if new_signal == old_signal {
            return;
        }
        inner.signal_callbacks.retain(|f| !f(new_signal));
    }

    /// Assert `signal`.
    pub fn signal_set(&self, signal: Signal) {
        self.signal_change(Signal::empty(), signal);
    }

    /// Deassert `signal`.
    pub fn signal_clear(&self, signal: Signal) {
        self.signal_change(signal, Signal::empty());
    }

    /// Add `callback` for signal status changes.
    ///
    /// The `callback` is a function of `Fn(Signal) -> bool`.
    /// It returns a bool indicating whether the handle process is over.
    /// If true, the function will never be called again.
    pub fn add_signal_callback(&self, callback: SignalHandler) {
        let mut inner = self.inner.lock();
        // Check the callback immediately, in case that a signal arrives just before the call of
        // `add_signal_callback` (since lock is acquired inside it) and the callback is not triggered
        // in time.
        if !callback(inner.signal) {
            if inner.signal_callbacks.len() >= MAX_SIGNAL_CALLBACKS {
                return;
            }
            inner.signal_callbacks.push(callback);
        }
    }
}

impl dyn KernelObject {
    /// Asynchronous wait for one of `signal`.
    pub fn wait_signal(self: &Arc<Self>, signal: Signal) -> impl Future<Output = Signal> {
        #[must_use = "wait_signal does nothing unless polled/`await`-ed"]
        struct SignalFuture {
            object: Arc<dyn KernelObject>,
            signal: Signal,
            first: bool,
        }

        impl Future for SignalFuture {
            type Output = Signal;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let current_signal = self.object.signal();
                if !(current_signal & self.signal).is_empty() {
                    return Poll::Ready(current_signal);
                }
                if self.first {
                    self.object.add_signal_callback(Box::new({
                        let signal = self.signal;
                        let waker = cx.waker().clone();
                        move |s| {
                            if (s & signal).is_empty() {
                                return false;
                            }
                            waker.wake_by_ref();
                            true
                        }
                    }));
                    self.first = false;
                }
                Poll::Pending
            }
        }

        SignalFuture {
            object: self.clone(),
            signal,
            first: true,
        }
    }

    /// Once one of the `signal` asserted, push a packet with `key` into the `port`,
    ///
    /// It's used to implement `sys_object_wait_async`.
    #[allow(unsafe_code)]
    pub fn send_signal_to_port_async(self: &Arc<Self>, signal: Signal, port: &Arc<Port>, key: u64) {
        let current_signal = self.signal();
        if !(current_signal & signal).is_empty() {
            port.push(PortPacketRepr {
                key,
                status: ZxError::OK,
                data: PayloadRepr::Signal(PacketSignal {
                    trigger: signal,
                    observed: current_signal,
                    count: 1,
                    timestamp: 0,
                    _reserved1: 0,
                }),
            });
            return;
        }
        self.add_signal_callback(Box::new({
            let port = port.clone();
            move |s| {
                if (s & signal).is_empty() {
                    return false;
                }
                port.push(PortPacketRepr {
                    key,
                    status: ZxError::OK,
                    data: PayloadRepr::Signal(PacketSignal {
                        trigger: signal,
                        observed: s,
                        count: 1,
                        timestamp: 0,
                        _reserved1: 0,
                    }),
                });
                true
            }
        }));
    }
}

/// Asynchronous wait signal for multiple objects.
pub fn wait_signal_many(
    targets: &[(Arc<dyn KernelObject>, Signal)],
) -> impl Future<Output = Vec<Signal>> {
    #[must_use = "wait_signal_many does nothing unless polled/`await`-ed"]
    struct SignalManyFuture {
        targets: Vec<(Arc<dyn KernelObject>, Signal)>,
        first: bool,
    }

    impl SignalManyFuture {
        fn happened(&self, current_signals: &[Signal]) -> bool {
            self.targets
                .iter()
                .zip(current_signals)
                .any(|(&(_, desired), &current)| !(current & desired).is_empty())
        }
    }

    impl Future for SignalManyFuture {
        type Output = Vec<Signal>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let current_signals: Vec<_> =
                self.targets.iter().map(|(obj, _)| obj.signal()).collect();
            if self.happened(&current_signals) {
                return Poll::Ready(current_signals);
            }
            if self.first {
                for (object, signal) in self.targets.iter() {
                    object.add_signal_callback(Box::new({
                        let signal = *signal;
                        let waker = cx.waker().clone();
                        move |s| {
                            if (s & signal).is_empty() {
                                return false;
                            }
                            waker.wake_by_ref();
                            true
                        }
                    }));
                }
                self.first = false;
            }
            Poll::Pending
        }
    }

    SignalManyFuture {
        targets: Vec::from(targets),
        first: true,
    }
}

/// Macro to auto implement `KernelObject` trait.
#[macro_export]
macro_rules! impl_kobject {
    ($class:ident $( $fn:tt )*) => {
        impl $crate::object::KernelObject for $class {
            fn id(&self) -> KoID {
                self.base.id
            }
            fn type_name(&self) -> &str {
                stringify!($class)
            }
            fn name(&self) -> alloc::string::String {
                self.base.name()
            }
            fn set_name(&self, name: &str){
                self.base.set_name(name)
            }
            fn signal(&self) -> Signal {
                self.base.signal()
            }
            fn signal_set(&self, signal: Signal) {
                self.base.signal_set(signal);
            }
            fn signal_clear(&self, signal: Signal) {
                self.base.signal_clear(signal);
            }
            fn signal_change(&self, clear: Signal, set: Signal) {
                self.base.signal_change(clear, set);
            }
            fn add_signal_callback(&self, callback: $crate::object::SignalHandler) {
                self.base.add_signal_callback(callback);
            }
            $( $fn )*
        }
        impl core::fmt::Debug for $class {
            fn fmt(
                &self,
                f: &mut core::fmt::Formatter<'_>,
            ) -> core::result::Result<(), core::fmt::Error> {
                use $crate::object::KernelObject;
                f.debug_tuple(&stringify!($class))
                    .field(&self.id())
                    .field(&self.name())
                    .finish()
            }
        }
    };
}

/// Define a pair of kcounter (create, destroy),
/// and a helper struct `CountHelper` which increases the counter on construction and drop.
#[macro_export]
macro_rules! define_count_helper {
    ($class:ident) => {
        struct CountHelper(());
        impl CountHelper {
            fn new() -> Self {
                $crate::kcounter!(CREATE_COUNT, concat!(stringify!($class), ".create"));
                CREATE_COUNT.add(1);
                CountHelper(())
            }
        }
        impl Drop for CountHelper {
            fn drop(&mut self) {
                $crate::kcounter!(DESTROY_COUNT, concat!(stringify!($class), ".destroy"));
                DESTROY_COUNT.add(1);
            }
        }
    };
}

/// The type of kernel object ID.
pub type KoID = u64;

/// The type of kernel object signal handler.
pub type SignalHandler = Box<dyn Fn(Signal) -> bool + Send>;

/// Empty kernel object. Just for test.
pub struct DummyObject {
    base: KObjectBase,
}

impl_kobject!(DummyObject);

impl DummyObject {
    /// Create a new `DummyObject`.
    pub fn new() -> Arc<Self> {
        Arc::new(DummyObject {
            base: KObjectBase::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_std::sync::Barrier;
    use std::time::Duration;

    #[async_std::test]
    async fn wait() {
        let object = DummyObject::new();
        let barrier = Arc::new(Barrier::new(2));
        async_std::task::spawn({
            let object = object.clone();
            let barrier = barrier.clone();
            async move {
                async_std::task::sleep(Duration::from_millis(20)).await;

                // Assert an irrelevant signal to test the `false` branch of the callback for `READABLE`.
                object.signal_set(Signal::USER_SIGNAL_0);
                object.signal_clear(Signal::USER_SIGNAL_0);
                object.signal_set(Signal::READABLE);
                barrier.wait().await;

                object.signal_set(Signal::WRITABLE);
            }
        });
        let object: Arc<dyn KernelObject> = object;

        let signal = object.wait_signal(Signal::READABLE).await;
        assert_eq!(signal, Signal::READABLE);
        barrier.wait().await;

        let signal = object.wait_signal(Signal::WRITABLE).await;
        assert_eq!(signal, Signal::READABLE | Signal::WRITABLE);
    }

    #[async_std::test]
    async fn wait_many() {
        let objs = [DummyObject::new(), DummyObject::new()];
        let barrier = Arc::new(Barrier::new(2));
        async_std::task::spawn({
            let objs = objs.clone();
            let barrier = barrier.clone();
            async move {
                async_std::task::sleep(Duration::from_millis(20)).await;

                objs[0].signal_set(Signal::READABLE);
                barrier.wait().await;

                objs[1].signal_set(Signal::WRITABLE);
            }
        });
        let obj0: Arc<dyn KernelObject> = objs[0].clone();
        let obj1: Arc<dyn KernelObject> = objs[1].clone();

        let signals = wait_signal_many(&[
            (obj0.clone(), Signal::READABLE),
            (obj1.clone(), Signal::READABLE),
        ])
        .await;
        assert_eq!(signals, [Signal::READABLE, Signal::empty()]);
        barrier.wait().await;

        let signals = wait_signal_many(&[
            (obj0.clone(), Signal::WRITABLE),
            (obj1.clone(), Signal::WRITABLE),
        ])
        .await;
        assert_eq!(signals, [Signal::READABLE, Signal::WRITABLE]);
    }

    #[test]
    fn test_trait_with_dummy() {
        let dummy = DummyObject::new();
        assert_eq!(dummy.name(), String::from(""));
        dummy.set_name("test");
        assert_eq!(dummy.name(), String::from("test"));
        dummy.signal_set(Signal::WRITABLE);
        assert_eq!(dummy.signal(), Signal::WRITABLE);
        dummy.signal_change(Signal::WRITABLE, Signal::READABLE);
        assert_eq!(dummy.signal(), Signal::READABLE);

        assert_eq!(dummy.get_child(0).unwrap_err(), ZxError::WRONG_TYPE);
        assert_eq!(dummy.peer().unwrap_err(), ZxError::NOT_SUPPORTED);
        assert_eq!(dummy.related_koid(), 0);
        assert_eq!(dummy.allowed_signals(), Signal::USER_ALL);

        assert_eq!(
            format!("{:?}", dummy),
            format!("DummyObject({}, \"test\")", dummy.id())
        );
    }
}

// ── [diag] Wild-write hunt: watch a process name's buffer ────────────────────
//
// A page-fault report that names the running process as `"l\u{fffd}"` says the
// name `String`'s heap buffer was overwritten: names are UTF-8 by construction
// and are written once, then only read. That makes the buffer an ideal target
// for a hardware write-watch — silent under normal operation, so the first
// trap it takes belongs to the corruptor described in
// `docs/README-crash-repro.md`, with its RIP in the trap frame.
//
// Arming is opt-in and one-shot: call `watch_process_name("lunarbar")`, and
// the next kernel object whose name starts with that prefix gets DR0 pointed
// at its name buffer on every CPU.

/// Requested name prefix, packed little-endian into 8 bytes (0 = no request).
/// An atomic rather than a `Mutex<String>`: this is read on the object-naming
/// path, which already holds the object lock and must not allocate.
static WATCH_NAME: AtomicU64 = AtomicU64::new(0);

/// Ask for the next kernel object whose name starts with `prefix` to have its
/// name buffer covered by a hardware write watchpoint. The prefix is capped at
/// 8 bytes (enough for a comm-style name); returns false if it is empty or
/// contains a NUL, which would truncate the packing.
pub fn watch_process_name(prefix: &str) -> bool {
    let b = prefix.as_bytes();
    if b.is_empty() || b.contains(&0) {
        return false;
    }
    let mut packed = [0u8; 8];
    let n = b.len().min(8);
    packed[..n].copy_from_slice(&b[..n]);
    WATCH_NAME.store(u64::from_le_bytes(packed), Ordering::Relaxed);
    true
}

/// Stop looking for a name to watch, and disarm any watchpoint already set.
pub fn unwatch_process_name() {
    WATCH_NAME.store(0, Ordering::Relaxed);
    kernel_hal::watchpoint::clear_watch();
}

/// If `name` matches the requested prefix, point the watchpoint at its buffer
/// and consume the request (one-shot: re-arming on every later object would
/// keep moving the watch off the buffer we want observed).
fn watch_name_if_requested(name: &str) {
    let packed = WATCH_NAME.load(Ordering::Relaxed);
    if packed == 0 {
        return;
    }
    let bytes = packed.to_le_bytes();
    let len = bytes.iter().position(|&c| c == 0).unwrap_or(8);
    if name.as_bytes().len() < len || &name.as_bytes()[..len] != &bytes[..len] {
        return;
    }
    // Watch an 8-byte window, so a String shorter than that is skipped: its
    // window would overlap neighbouring allocations and make hits ambiguous.
    // x86 ignores a misaligned watchpoint rather than reporting an error, so
    // an unaligned buffer is skipped too — the next matching object is likely
    // to be better placed.
    let addr = name.as_ptr() as usize;
    if name.len() < 8 || addr % 8 != 0 {
        return;
    }
    if kernel_hal::watchpoint::watch_write(addr, 8) {
        WATCH_NAME.store(0, Ordering::Relaxed);
        warn!("[watchpoint] armed on 8B at {:#x} — name buffer of \"{}\"", addr, name);
    }
}
