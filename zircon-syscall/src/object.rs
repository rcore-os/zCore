use {super::*, zircon_object::task::*};

const ZX_PROP_NAME: u32 = 3;
const ZX_MAX_NAME_LEN: u32 = 32;

impl Syscall {
    pub fn sys_object_get_property(
        &self,
        handle_value: HandleValue,
        property: u32,
        mut ptr: UserOutPtr<u8>,
        buffer_size: u32,
    ) -> ZxResult<usize> {
        info!(
            "handle={:?}, property={:?}, buffer_ptr={:?}, size={:?}",
            handle_value, property, ptr, buffer_size
        );
        let object = self
            .thread
            .proc()
            .get_dyn_object_with_rights(handle_value, Rights::GET_PROPERTY)?;
        match property {
            ZX_PROP_NAME => {
                if buffer_size < ZX_MAX_NAME_LEN {
                    Err(ZxError::BUFFER_TOO_SMALL)
                } else {
                    let s = object.name();
                    info!("object_get_property: name is {}", s);
                    ptr.write_cstring(s.as_str())
                        .expect("failed to write cstring");
                    Ok(0)
                }
            }
            _ => {
                warn!("unknown property {} in OBJECT_GET_PROPERTY", property);
                Err(ZxError::INVALID_ARGS)
            }
        }
    }

    pub fn sys_object_set_property(
        &self,
        handle_value: HandleValue,
        property: u32,
        ptr: UserInPtr<u8>,
        buffer_size: u32,
    ) -> ZxResult<usize> {
        info!(
            "handle={:?}, property={:?}, buffer_ptr={:?}, size={:?}",
            handle_value, property, ptr, buffer_size
        );
        let object = self
            .thread
            .proc()
            .get_dyn_object_with_rights(handle_value, Rights::SET_PROPERTY)?;
        match property {
            ZX_PROP_NAME => {
                let length = if buffer_size > ZX_MAX_NAME_LEN {
                    (ZX_MAX_NAME_LEN - 1) as usize
                } else {
                    buffer_size as usize
                };
                let s = ptr.read_string(length)?;
                info!("object_set_property name: {}", s);
                object.set_name(&s);
                Ok(0)
            }
            _ => {
                warn!("unknown property {} in OBJECT_GET_PROPERTY", property);
                Err(ZxError::INVALID_ARGS)
            }
        }
    }

    pub async fn sys_object_wait_one(
        &self,
        handle: HandleValue,
        signals: u32,
        deadline: u64,
        mut observed: UserOutPtr<Signal>,
    ) -> ZxResult<usize> {
        info!(
            "object.wait_one: handle={:?}, signals={:#x?}, deadline={:#x?}, observed={:#x?}",
            handle, signals, deadline, observed
        );
        let signals = Signal::from_bits(signals).ok_or(ZxError::INVALID_ARGS)?;
        let object = self
            .thread
            .proc()
            .get_dyn_object_with_rights(handle, Rights::WAIT)?;
        observed.write(object.wait_signal_async(signals).await)?;
        Ok(0)
    }

    pub fn sys_object_get_info(
        &self,
        handle: HandleValue,
        topic: u32,
        buffer: usize,
        _buffer_size: usize,
        _actual: UserOutPtr<usize>,
        _avail: UserOutPtr<usize>,
    ) -> ZxResult<usize> {
        match ZxInfo::from(topic) {
            ZxInfo::InfoProcess => {
                let proc = self
                    .thread
                    .proc()
                    .get_object_with_rights::<Process>(handle, Rights::INSPECT)?;
                UserOutPtr::<ProcessInfo>::from(buffer).write(proc.get_info())?;
            }
            _ => {
                warn!("not supported info topic");
                return Err(ZxError::NOT_SUPPORTED);
            }
        }
        Ok(0)
    }
}

#[repr(u32)]
enum ZxInfo {
    InfoNone = 0u32,
    InfoHandleValid = 1u32,
    InfoHandleBasic = 2u32,
    InfoProcess = 3u32,
    InfoProcessThreads = 4u32,
    InfoVmar = 7u32,
    InfoJobChildren = 8u32,
    InfoJobProcess = 9u32,
    InfoThread = 10u32,
    InfoThreadExceptionReport = 11u32,
    InfoTaskStats = 12u32,
    InfoProcessMaps = 13u32,
    InfoProcessVmos = 14u32,
    InfoThreadStats = 15u32,
    InfoCpuStats = 16u32,
    InfoKmemStats = 17u32,
    InfoResource = 18u32,
    InfoHandleCount = 19u32,
    InfoBti = 20u32,
    InfoProcessHandleStats = 21u32,
    InfoSocket = 22u32,
    InfoVmo = 23u32,
    InfoJob = 24u32,
    InfoTimer = 26u32,
    InfoStream = 27u32,
    Unknown,
}

impl From<u32> for ZxInfo {
    fn from(number: u32) -> Self {
        match number {
            0 => ZxInfo::InfoNone,
            1 => ZxInfo::InfoHandleValid,
            2 => ZxInfo::InfoHandleBasic,
            3 => ZxInfo::InfoProcess,
            4 => ZxInfo::InfoProcessThreads,
            7 => ZxInfo::InfoVmar,
            8 => ZxInfo::InfoJobChildren,
            9 => ZxInfo::InfoJobProcess,
            10 => ZxInfo::InfoThread,
            11 => ZxInfo::InfoThreadExceptionReport,
            12 => ZxInfo::InfoTaskStats,
            13 => ZxInfo::InfoProcessMaps,
            14 => ZxInfo::InfoProcessVmos,
            15 => ZxInfo::InfoThreadStats,
            16 => ZxInfo::InfoCpuStats,
            17 => ZxInfo::InfoKmemStats,
            18 => ZxInfo::InfoResource,
            19 => ZxInfo::InfoHandleCount,
            20 => ZxInfo::InfoBti,
            21 => ZxInfo::InfoProcessHandleStats,
            22 => ZxInfo::InfoSocket,
            23 => ZxInfo::InfoVmo,
            24 => ZxInfo::InfoJob,
            26 => ZxInfo::InfoTimer,
            27 => ZxInfo::InfoStream,
            _ => ZxInfo::Unknown,
        }
    }
}
