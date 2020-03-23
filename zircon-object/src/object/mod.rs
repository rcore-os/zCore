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
//! use spin::Mutex;
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
//! [`Channel`]: crate::ipc::Channel_
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
    spin::Mutex,
};

pub use {super::*, handle::*, rights::*, signal::*};

mod handle;
mod rights;
mod signal;

/// Common interface of a kernel object.
///
/// Implemented by [`impl_kobject`] macro.
///
/// [`impl_kobject`]: impl_kobject
pub trait KernelObject: DowncastSync + Debug {
    fn id(&self) -> KoID;
    fn obj_type(&self) -> ObjectType;
    fn name(&self) -> alloc::string::String;
    fn set_name(&self, name: &str);
    fn signal(&self) -> Signal;
    fn signal_set(&self, signal: Signal);
    fn add_signal_callback(&self, callback: SignalHandler);
    fn get_child(&self, _id: KoID) -> ZxResult<Arc<dyn KernelObject>> {
        Err(ZxError::WRONG_TYPE)
    }
    fn related_koid(&self) -> KoID {
        0u64
    }
    fn get_info(&self, h_info: &mut HandleBasicInfo) {
        h_info.koid = self.id();
        h_info.obj_type = self.obj_type() as u32;
        h_info.related_koid = self.related_koid();
    }
    fn user_signal_peer(&self, _clear: Signal, _set: Signal) -> ZxResult<()> {
        Err(ZxError::NOT_SUPPORTED)
    }
}

impl_downcast!(sync KernelObject);

/// The base struct of a kernel object.
pub struct KObjectBase {
    pub id: KoID,
    inner: Mutex<KObjectBaseInner>,
}

/// The mutable part of `KObjectBase`.
struct KObjectBaseInner {
    name: String,
    signal: Signal,
    signal_callbacks: Vec<SignalHandler>,
}

impl Default for KObjectBaseInner {
    fn default() -> Self {
        KObjectBaseInner {
            name: {
                let mut s = String::with_capacity(32);
                for _ in 0..32 {
                    s.push('\0');
                }
                s
            },
            signal: Signal::default(),
            signal_callbacks: Vec::default(),
        }
    }
}

impl Default for KObjectBase {
    fn default() -> Self {
        KObjectBase {
            id: Self::new_koid(),
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
        KObjectBase {
            id: Self::new_koid(),
            inner: Mutex::new(KObjectBaseInner {
                signal,
                ..Default::default()
            }),
        }
    }

    /// Create a kernel object base with `name`.
    pub fn with_name(name: &str) -> Self {
        KObjectBase {
            id: Self::new_koid(),
            inner: Mutex::new(KObjectBaseInner {
                name: String::from(name),
                ..Default::default()
            }),
        }
    }

    /// Generate a new KoID.
    fn new_koid() -> KoID {
        static KOID: AtomicU64 = AtomicU64::new(1024);
        KOID.fetch_add(1, Ordering::SeqCst)
    }

    /// Get object's name.
    pub fn name(&self) -> String {
        self.inner.lock().name.clone()
    }

    /// Set object's name.
    pub fn set_name(&self, name: &str) {
        let s = &mut self.inner.lock().name;
        s.clear();
        assert!(name.len() <= 32, "name is too long for object");
        s.push_str(name);
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
        inner.signal_callbacks.push(callback);
    }
}

impl dyn KernelObject {
    /// Asynchronous wait for one of `signal`.
    pub fn wait_signal_async(self: &Arc<Self>, signal: Signal) -> impl Future<Output = Signal> {
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
            let packet_payload = PortPacketSignal {
                trigger: signal,
                observed: current_signal,
                count: 1u64,
                timestamp: 0u64,
                reserved1: 0u64,
            };
            port.push(PortPacket {
                key,
                _type: PortPacketType::SignalOne,
                status: ZxError::OK,
                data: unsafe { core::mem::transmute(packet_payload) },
            });
            return;
        }
        self.add_signal_callback(Box::new({
            let port = port.clone();
            move |s| {
                if (s & signal).is_empty() {
                    return false;
                }
                let packet_payload = PortPacketSignal {
                    trigger: signal,
                    observed: s,
                    count: 1u64,
                    timestamp: 0u64,
                    reserved1: 0u64,
                };
                port.push(PortPacket {
                    key,
                    _type: PortPacketType::SignalOne,
                    status: ZxError::OK,
                    data: unsafe { core::mem::transmute(packet_payload) },
                });
                true
            }
        }));
    }
}

/// Asynchronous wait signal for multiple objects.
pub fn wait_signal_many_async(
    targets: &[(Arc<dyn KernelObject>, Signal)],
) -> impl Future<Output = Vec<Signal>> {
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
        impl KernelObject for $class {
            fn id(&self) -> KoID {
                self.base.id
            }
            fn obj_type(&self) -> $crate::object::ObjectType {
                ObjectType::$class
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
            fn add_signal_callback(&self, callback: SignalHandler) {
                self.base.add_signal_callback(callback);
            }
            $( $fn )*
        }
        impl core::fmt::Debug for $class {
            fn fmt(
                &self,
                f: &mut core::fmt::Formatter<'_>,
            ) -> core::result::Result<(), core::fmt::Error> {
                f.debug_tuple("KObject")
                    .field(&self.id())
                    .field(&self.name())
                    .finish()
            }
        }
    };
}

/// The type of kernel object ID.
pub type KoID = u64;

/// The object type
#[repr(u32)]
#[derive(Debug)]
pub enum ObjectType {
    None = 0,
    Process = 1,
    Thread = 2,
    Vmo = 3,
    Channel = 4,
    Event = 5,
    Port = 6,
    Interrupt = 9,
    PciDevice = 11,
    Log = 12,
    Socket = 14,
    Resource = 15,
    EventPair = 16,
    Job = 17,
    Vmar = 18,
    Fifo = 19,
    Guest = 20,
    VCpu = 21,
    Timer = 22,
    Iommu = 23,
    Bti = 24,
    Profile = 25,
    Pmt = 26,
    SuspendToken = 27,
    Pager = 28,
    Exception = 29,
    Clock = 30,
    Stream = 31,
}

#[allow(non_upper_case_globals)]
impl ObjectType {
    pub const DummyObject: ObjectType = ObjectType::None;
    pub const DebugLog: ObjectType = ObjectType::Log;
    pub const Futex: ObjectType = ObjectType::None; // TODO
    pub const VmAddressRegion: ObjectType = ObjectType::Vmar;
    pub const VmObject: ObjectType = ObjectType::Vmo;

    // workaround for objects on linux
    pub const File: ObjectType = ObjectType::None;
}

/// The type of kernel object signal handler.
pub type SignalHandler = Box<dyn Fn(Signal) -> bool + Send>;

/// Empty kernel object. Just for test.
pub struct DummyObject {
    base: KObjectBase,
}

impl_kobject!(DummyObject);

impl DummyObject {
    pub fn new() -> Arc<Self> {
        Arc::new(DummyObject {
            base: KObjectBase::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[async_std::test]
    async fn wait_async() {
        let object = DummyObject::new();
        let flag = Arc::new(AtomicU8::new(0));

        async_std::task::spawn({
            let object = object.clone();
            let flag = flag.clone();
            async move {
                flag.store(1, Ordering::SeqCst);
                object.base.signal_set(Signal::READABLE);
                async_std::task::sleep(Duration::from_millis(10)).await;

                flag.store(2, Ordering::SeqCst);
                object.base.signal_set(Signal::WRITABLE);
            }
        });
        let object: Arc<dyn KernelObject> = object;
        assert_eq!(flag.load(Ordering::SeqCst), 0);

        let signal = object.wait_signal_async(Signal::READABLE).await;
        assert_eq!(signal, Signal::READABLE);
        assert_eq!(flag.load(Ordering::SeqCst), 1);

        let signal = object.wait_signal_async(Signal::WRITABLE).await;
        assert_eq!(signal, Signal::READABLE | Signal::WRITABLE);
        assert_eq!(flag.load(Ordering::SeqCst), 2);
    }

    #[async_std::test]
    async fn wait_many_async() {
        let objs = [DummyObject::new(), DummyObject::new()];
        let flag = Arc::new(AtomicU8::new(0));

        async_std::task::spawn({
            let objs = objs.clone();
            let flag = flag.clone();
            async move {
                flag.store(1, Ordering::SeqCst);
                objs[0].base.signal_set(Signal::READABLE);
                async_std::task::sleep(Duration::from_millis(1)).await;

                flag.store(2, Ordering::SeqCst);
                objs[1].base.signal_set(Signal::WRITABLE);
            }
        });
        let obj0: Arc<dyn KernelObject> = objs[0].clone();
        let obj1: Arc<dyn KernelObject> = objs[1].clone();
        assert_eq!(flag.load(Ordering::SeqCst), 0);

        let signals = wait_signal_many_async(&[
            (obj0.clone(), Signal::READABLE),
            (obj1.clone(), Signal::READABLE),
        ])
        .await;
        assert_eq!(signals, [Signal::READABLE, Signal::empty()]);
        assert_eq!(flag.load(Ordering::SeqCst), 1);

        let signals = wait_signal_many_async(&[
            (obj0.clone(), Signal::WRITABLE),
            (obj1.clone(), Signal::WRITABLE),
        ])
        .await;
        assert_eq!(signals, [Signal::READABLE, Signal::WRITABLE]);
        assert_eq!(flag.load(Ordering::SeqCst), 2);
    }
}
