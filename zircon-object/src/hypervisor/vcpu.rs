use {
    crate::{
        hypervisor::{Guest, VcpuIo, VcpuState},
        object::*,
        signal::*,
        task::{Thread, ThreadFlag},
    },
    alloc::sync::Arc,
    core::convert::TryInto,
    lock::Mutex,
    rvm::{self, Vcpu as VcpuInner},
};

/// Virtual CPU within a Guest, which allows for execution within the virtual machine.
pub struct Vcpu {
    base: KObjectBase,
    _counter: CountHelper,
    thread: Arc<Thread>,
    inner: Mutex<VcpuInner>,
}

impl_kobject!(Vcpu);
define_count_helper!(Vcpu);

impl Vcpu {
    /// Create a new VCPU within a guest.
    pub fn new(guest: Arc<Guest>, entry: u64, thread: Arc<Thread>) -> ZxResult<Arc<Self>> {
        if thread.flags().contains(ThreadFlag::VCPU) {
            return Err(ZxError::BAD_STATE);
        }
        let inner = Mutex::new(VcpuInner::new(entry, guest.rvm_guest())?);
        thread.update_flags(|flags| flags.insert(ThreadFlag::VCPU));
        Ok(Arc::new(Vcpu {
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            thread,
            inner,
        }))
    }

    /// Check whether `current_thread` is the thread of the VCPU.
    pub fn same_thread(&self, current_thread: &Arc<Thread>) -> bool {
        Arc::ptr_eq(&self.thread, current_thread)
    }

    /// Inject a virtual interrupt.
    pub fn virtual_interrupt(&self, vector: u32) -> ZxResult {
        self.inner
            .lock()
            .virtual_interrupt(vector)
            .map_err(From::from)
    }

    /// Resume execution of the VCPU.
    pub fn resume(&self) -> ZxResult<PortPacket> {
        self.inner.lock().resume()?.try_into()
    }

    /// Read state from the VCPU.
    pub fn read_state(&self) -> ZxResult<VcpuState> {
        self.inner.lock().read_state().map_err(From::from)
    }

    /// Write state to the VCPU.
    pub fn write_state(&self, state: &VcpuState) -> ZxResult {
        self.inner.lock().write_state(state).map_err(From::from)
    }

    /// Write IO state to the VCPU.
    pub fn write_io_state(&self, state: &VcpuIo) -> ZxResult {
        self.inner.lock().write_io_state(state).map_err(From::from)
    }
}

impl Drop for Vcpu {
    fn drop(&mut self) {
        self.thread
            .update_flags(|flags| flags.remove(ThreadFlag::VCPU));
    }
}

impl From<rvm::BellPacket> for PacketGuestBell {
    fn from(bell: rvm::BellPacket) -> Self {
        Self {
            addr: bell.addr,
            ..Default::default()
        }
    }
}

impl From<rvm::IoPacket> for PacketGuestIo {
    fn from(io: rvm::IoPacket) -> Self {
        Self {
            port: io.port,
            access_size: io.access_size,
            input: io.input,
            data: io.data,
            ..Default::default()
        }
    }
}

impl From<rvm::MmioPacket> for PacketGuestMem {
    fn from(mem: rvm::MmioPacket) -> Self {
        #[cfg(target_arch = "x86_64")]
        Self {
            addr: mem.addr,
            inst_len: mem.inst_len,
            inst_buf: mem.inst_buf,
            default_operand_size: mem.default_operand_size,
            ..Default::default()
        }
    }
}

impl TryInto<PortPacket> for rvm::RvmExitPacket {
    type Error = ZxError;

    #[allow(unsafe_code)]
    fn try_into(self) -> ZxResult<PortPacket> {
        use rvm::RvmExitPacketKind;
        let data = match self.kind {
            RvmExitPacketKind::GuestBell => {
                PayloadRepr::GuestBell(unsafe { self.inner.bell.into() })
            }
            RvmExitPacketKind::GuestIo => PayloadRepr::GuestIo(unsafe { self.inner.io.into() }),
            RvmExitPacketKind::GuestMmio => {
                PayloadRepr::GuestMem(unsafe { self.inner.mmio.into() })
            }
            _ => return Err(ZxError::NOT_SUPPORTED),
        };
        Ok(PortPacketRepr {
            key: self.key,
            status: ZxError::OK,
            data,
        }
        .into())
    }
}
