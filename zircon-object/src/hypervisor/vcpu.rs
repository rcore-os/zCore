use {
    crate::{hypervisor::Guest, object::*, signal::*},
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
        self.inner.lock().resume()?.try_into()
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
        match self.kind {
            RvmExitPacketKind::GuestIo => Ok(PortPacketRepr {
                key: self.key,
                status: ZxError::OK,
                data: PayloadRepr::GuestIo(unsafe { self.inner.io.into() }),
            }
            .into()),
            RvmExitPacketKind::GuestMmio => Ok(PortPacketRepr {
                key: self.key,
                status: ZxError::OK,
                data: PayloadRepr::GuestMem(unsafe { self.inner.mmio.into() }),
            }
            .into()),
            _ => Err(ZxError::NOT_SUPPORTED),
        }
    }
}
