use {
    crate::{
        hypervisor::{Guest, VcpuIo, VcpuState},
        object::*,
        signal::*,
    },
    alloc::sync::Arc,
    core::convert::TryInto,
    rvm::{self, Vcpu as VcpuInner},
    spin::Mutex,
};

pub struct Vcpu {
    base: KObjectBase,
    _counter: CountHelper,
    inner: Mutex<VcpuInner>,
}

impl_kobject!(Vcpu);
define_count_helper!(Vcpu);

impl Vcpu {
    pub fn new(guest: Arc<Guest>, entry: u64) -> ZxResult<Arc<Self>> {
        // TODO: check thread
        Ok(Arc::new(Vcpu {
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            inner: Mutex::new(VcpuInner::new(entry, guest.rvm_geust())?),
        }))
    }

    pub fn virtual_interrupt(&self, vector: u32) -> ZxResult {
        self.inner
            .lock()
            .virtual_interrupt(vector)
            .map_err(From::from)
    }

    pub fn resume(&self) -> ZxResult<PortPacket> {
        // TODO: check thread
        self.inner.lock().resume()?.try_into()
    }

    pub fn read_state(&self) -> ZxResult<VcpuState> {
        // TODO: check thread
        self.inner.lock().read_state().map_err(From::from)
    }

    pub fn write_state(&self, state: &VcpuState) -> ZxResult {
        // TODO: check thread
        self.inner.lock().write_state(state).map_err(From::from)
    }

    pub fn write_io_state(&self, state: &VcpuIo) -> ZxResult {
        // TODO: check thread
        self.inner.lock().write_io_state(state).map_err(From::from)
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
