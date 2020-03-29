use {
    super::*,
    core::convert::TryFrom,
    numeric_enum_macro::numeric_enum,
    zircon_object::{signal::Port, task::*, vm::*},
    alloc::vec::Vec,
};

impl Syscall<'_> {
    pub fn sys_object_get_property(
        &self,
        handle_value: HandleValue,
        property: u32,
        ptr: usize,
        buffer_size: u32,
    ) -> ZxResult {
        let property = Property::try_from(property).map_err(|_| ZxError::INVALID_ARGS)?;
        info!(
            "object.get_property: handle={:#x?}, property={:?}, buffer=({:#x}; {:#x?})",
            handle_value, property, ptr, buffer_size
        );
        let proc = self.thread.proc();
        let object = proc.get_dyn_object_with_rights(handle_value, Rights::GET_PROPERTY)?;
        match property {
            Property::Name => {
                if buffer_size < MAX_NAME_LEN {
                    return Err(ZxError::BUFFER_TOO_SMALL);
                }
                let s = object.name();
                info!("name={:?}", s);
                UserOutPtr::<u8>::from(ptr)
                    .write_cstring(s.as_str())
                    .expect("failed to write cstring");
                Ok(())
            }
            Property::ProcessDebugAddr => {
                if buffer_size < 8 {
                    return Err(ZxError::BUFFER_TOO_SMALL);
                }
                let debug_addr = proc
                    .get_object_with_rights::<Process>(handle_value, Rights::GET_PROPERTY)?
                    .get_debug_addr();
                UserOutPtr::<usize>::from(ptr).write(debug_addr)?;
                Ok(())
            }
            Property::ProcessVdsoBaseAddress => {
                if buffer_size < 8 {
                    return Err(ZxError::BUFFER_TOO_SMALL);
                }
                let vdso_base = proc.vmar().vdso_base_addr().unwrap_or(0);
                info!("vdso_base_addr={:#X}", vdso_base);
                UserOutPtr::<usize>::from(ptr).write(vdso_base)?;
                Ok(())
            }
            Property::ProcessBreakOnLoad => {
                if buffer_size < 8 {
                    return Err(ZxError::BUFFER_TOO_SMALL);
                }
                let break_on_load = proc
                    .get_object_with_rights::<Process>(handle_value, Rights::GET_PROPERTY)?
                    .get_dyn_break_on_load();
                UserOutPtr::<usize>::from(ptr).write(break_on_load)?;
                Ok(())
            }
            _ => {
                warn!("unknown property");
                Err(ZxError::INVALID_ARGS)
            }
        }
    }

    pub fn sys_object_set_property(
        &mut self,
        handle_value: HandleValue,
        property: u32,
        ptr: usize,
        buffer_size: u32,
    ) -> ZxResult {
        let property = Property::try_from(property).map_err(|_| ZxError::INVALID_ARGS)?;
        info!(
            "object.set_property: handle={:#x?}, property={:?}, buffer=({:#x}; {:#x?})",
            handle_value, property, ptr, buffer_size
        );
        let proc = self.thread.proc();
        let object = proc.get_dyn_object_with_rights(handle_value, Rights::SET_PROPERTY)?;
        match property {
            Property::Name => {
                let length = buffer_size.min(MAX_NAME_LEN) as usize;
                let s = UserInPtr::<u8>::from(ptr).read_string(length)?;
                info!("set name={:?}", s);
                object.set_name(&s);
                Ok(())
            }
            Property::ProcessDebugAddr => {
                if buffer_size < 8 {
                    return Err(ZxError::BUFFER_TOO_SMALL);
                }
                let addr = UserInPtr::<usize>::from(ptr).read()?;
                proc.get_object_with_rights::<Process>(handle_value, Rights::SET_PROPERTY)?
                    .set_debug_addr(addr);
                Ok(())
            }
            Property::RegisterFs => {
                if buffer_size < 8 {
                    return Err(ZxError::BUFFER_TOO_SMALL);
                }
                let thread = proc.get_object::<Thread>(handle_value)?;
                assert!(Arc::ptr_eq(&thread, &self.thread));
                let fsbase = UserInPtr::<u64>::from(ptr).read()?;
                info!("set fsbase = {:#x}", fsbase);
                self.regs.fsbase = fsbase as usize;
                Ok(())
            }
            Property::ProcessBreakOnLoad => {
                if buffer_size < 8 {
                    return Err(ZxError::BUFFER_TOO_SMALL);
                }
                let addr = UserInPtr::<usize>::from(ptr).read()?;
                proc.get_object_with_rights::<Process>(handle_value, Rights::SET_PROPERTY)?
                    .set_dyn_break_on_load(addr);
                Ok(())
            }
            _ => {
                warn!("unknown property");
                Err(ZxError::INVALID_ARGS)
            }
        }
    }

    pub async fn sys_object_wait_one(
        &self,
        handle: HandleValue,
        signals: u32,
        deadline: i64,
        mut observed: UserOutPtr<Signal>,
    ) -> ZxResult {
        info!(
            "object.wait_one: handle={:#x?}, signals={:#x?}, deadline={:#x?}, observed={:#x?}",
            handle, signals, deadline, observed
        );
        let proc = self.thread.proc();
        let object = proc.get_dyn_object_with_rights(handle, Rights::WAIT)?;
        if deadline <= 0 {
            if let Some(signals) = Signal::from_bits(signals) {
                let res = object.signal();
                observed.write_if_not_null(res)?;
                if (signals & res).is_empty() {
                    Err(ZxError::TIMED_OUT)
                } else {
                    Ok(())
                }
            } else {
                Err(ZxError::TIMED_OUT)
            }
        } else {
            let signals = Signal::from_bits(signals).ok_or(ZxError::INVALID_ARGS)?;
            if deadline == i64::max_value() {
                observed.write_if_not_null(object.wait_signal(signals).await)?;
                Ok(())
            } else {
                unimplemented!()
            }
        }
    }

    pub fn sys_object_get_info(
        &self,
        handle: HandleValue,
        topic: u32,
        buffer: usize,
        buffer_size: usize,
        _actual: UserOutPtr<usize>,
        _avail: UserOutPtr<usize>,
    ) -> ZxResult {
        let topic = Topic::try_from(topic).map_err(|_| ZxError::INVALID_ARGS)?;
        info!(
            "object.get_info: handle={:#x?}, topic={:?}, buffer=({:#x}; {:#x})",
            handle, topic, buffer, buffer_size,
        );
        let proc = self.thread.proc();
        match topic {
            Topic::Process => {
                let proc = proc.get_object_with_rights::<Process>(handle, Rights::INSPECT)?;
                UserOutPtr::<ProcessInfo>::from(buffer).write(proc.get_info())?;
            }
            Topic::Vmar => {
                let vmar =
                    proc.get_object_with_rights::<VmAddressRegion>(handle, Rights::INSPECT)?;
                UserOutPtr::<VmarInfo>::from(buffer).write(vmar.get_info())?;
            }
            Topic::HandleBasic => {
                let info = proc.get_handle_info(handle)?;
                UserOutPtr::<HandleBasicInfo>::from(buffer).write(info)?;
            }
            Topic::Thread => {
                let info = proc.get_object_with_rights::<Thread>(handle, Rights::INSPECT)?.get_thread_info();
                UserOutPtr::<ThreadInfo>::from(buffer).write(info)?;
            }
            _ => {
                warn!("not supported info topic: {:?}", topic);
                return Err(ZxError::NOT_SUPPORTED);
            }
        }
        Ok(())
    }

    pub fn sys_object_signal_peer(
        &self,
        handle_value: HandleValue,
        clear_mask: u32,
        set_mask: u32,
    ) -> ZxResult {
        info!(
            "object.signal_peer: handle_value = {:#x}, clear_mask = {:#x}, set_mask = {:#x}",
            handle_value, clear_mask, set_mask
        );
        let proc = self.thread.proc();
        let object = proc.get_dyn_object_with_rights(handle_value, Rights::SIGNAL_PEER)?;
        let allowed_signals = object.allowed_signals();
        let clear_signal = Signal::verify_user_signal(allowed_signals, clear_mask)?;
        let set_signal = Signal::verify_user_signal(allowed_signals, set_mask)?;
        object.peer()?.signal_change(clear_signal, set_signal);
        Ok(())
    }

    pub fn sys_object_wait_async(
        &self,
        handle_value: HandleValue,
        port_handle_value: HandleValue,
        key: u64,
        signals: u32,
        options: u32,
    ) -> ZxResult {
        let signals = Signal::from_bits(signals).ok_or(ZxError::INVALID_ARGS)?;
        info!(
            "object.wait_async: handle={:#x}, port={:#x}, key={:#x}, signal={:?}, options={:#X}",
            handle_value, port_handle_value, key, signals, options
        );
        if options != 0 {
            unimplemented!()
        }
        // TODO filter `options`
        let proc = self.thread.proc();
        let object = proc.get_dyn_object_with_rights(handle_value, Rights::WAIT)?;
        let port = proc.get_object_with_rights::<Port>(port_handle_value, Rights::WRITE)?;
        object.send_signal_to_port_async(signals, &port, key);
        Ok(())
    }

    pub fn sys_object_signal(
        &self,
        handle_value: HandleValue,
        clear_mask: u32,
        set_mask: u32,
    ) -> ZxResult {
        info!(
            "object.signal: handle_value={:#x}, clear_mask={:#x}, set_mask={:#x}",
            handle_value, clear_mask, set_mask
        );
        let proc = self.thread.proc();
        let object = proc.get_dyn_object_with_rights(handle_value, Rights::SIGNAL)?;
        let allowed_signals = object.allowed_signals();
        info!("{:?} allowed: {:?}", object, allowed_signals);
        let clear_signal = Signal::verify_user_signal(allowed_signals, clear_mask)?;
        let set_signal = Signal::verify_user_signal(allowed_signals, set_mask)?;
        object.signal_change(clear_signal, set_signal);
        Ok(())
    }

    pub async fn sys_object_wait_many(
        &self,
        mut user_items: UserInOutPtr<UserWaitItem>,
        count: u32,
        deadline: u64
    ) -> ZxResult {
        if count > MAX_WAIT_MANY_ITEMS {
            return Err(ZxError::OUT_OF_RANGE);
        }
        let mut items = user_items.read_array(count as usize)?;
        info!("user_items: {:#x?}, deadline: {:#x}", user_items, deadline);
        let proc = self.thread.proc();
        let mut waiters = Vec::new();
        for item in items.iter() {
            let object = proc.get_dyn_object_with_rights(item.handle, Rights::WAIT)?;
            waiters.push((object, item.wait_for));
        }
        let res = wait_signal_many(&waiters).await;
        for (i, item) in items.iter_mut().enumerate() {
            item.observed = res[i];
        }
        user_items.write_array(&items)?;
        Ok(())
    }
}

numeric_enum! {
    #[repr(u32)]
    #[derive(Debug)]
    enum Topic {
        None = 0,
        HandleValid = 1,
        HandleBasic = 2,
        Process = 3,
        ProcessThreads = 4,
        Vmar = 7,
        JobChildren = 8,
        JobProcess = 9,
        Thread = 10,
        ThreadExceptionReport = 11,
        TaskStats = 12,
        ProcessMaps = 13,
        ProcessVmos = 14,
        ThreadStats = 15,
        CpuStats = 16,
        KmemStats = 17,
        Resource = 18,
        HandleCount = 19,
        Bti = 20,
        ProcessHandleStats = 21,
        Socket = 22,
        Vmo = 23,
        Job = 24,
        Timer = 26,
        Stream = 27,
    }
}

numeric_enum! {
    #[repr(u32)]
    #[derive(Debug)]
    enum Property {
        Name = 3,
        RegisterFs = 4,
        ProcessDebugAddr = 5,
        ProcessVdsoBaseAddress = 6,
        ProcessBreakOnLoad = 7,
    }
}

const MAX_NAME_LEN: u32 = 32;
const MAX_WAIT_MANY_ITEMS: u32 = 32;

#[derive(Debug)]
#[repr(C)]
pub struct UserWaitItem {
    handle: HandleValue,
    wait_for: Signal,
    observed: Signal,
}
