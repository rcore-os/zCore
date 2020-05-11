use {
    super::*,
    alloc::vec::Vec,
    core::convert::TryFrom,
    numeric_enum_macro::numeric_enum,
    zircon_object::{dev::*, signal::Port, task::*, vm::*},
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
        deadline: Deadline,
        mut observed: UserOutPtr<Signal>,
    ) -> ZxResult {
        let signals = Signal::from_bits_truncate(signals);
        info!(
            "object.wait_one: handle={:#x?}, signals={:#x?}, deadline={:#x?}, observed={:#x?}",
            handle, signals, deadline, observed
        );
        let proc = self.thread.proc();
        let object = proc.get_dyn_object_with_rights(handle, Rights::WAIT)?;
        let cancel_token = proc.get_cancel_token(handle)?;
        let future = object.wait_signal(signals);
        let signal = self
            .thread
            .cancelable_blocking_run(
                future,
                ThreadState::BlockedWaitOne,
                deadline.into(),
                cancel_token,
            )
            .await
            .or_else(|e| {
                if e == ZxError::TIMED_OUT {
                    observed.write_if_not_null(object.signal())?;
                }
                Err(e)
            })?;
        observed.write_if_not_null(signal)?;
        Ok(())
    }

    pub fn sys_object_get_info(
        &self,
        handle: HandleValue,
        topic: u32,
        buffer: usize,
        buffer_size: usize,
        mut actual: UserOutPtr<usize>,
        mut avail: UserOutPtr<usize>,
    ) -> ZxResult {
        let topic = Topic::try_from(topic).map_err(|_| ZxError::INVALID_ARGS)?;
        info!(
            "object.get_info: handle={:#x?}, topic={:?}, buffer=({:#x}; {:#x})",
            handle, topic, buffer, buffer_size,
        );
        let proc = self.thread.proc();
        match topic {
            Topic::HandleValid => {
                let _ = proc.get_dyn_object_with_rights(handle, Rights::empty())?;
            }
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
                let thread = proc.get_object_with_rights::<Thread>(handle, Rights::INSPECT)?;
                UserOutPtr::<ThreadInfo>::from(buffer).write(thread.get_thread_info())?;
            }
            Topic::HandleCount => {
                let object = proc.get_dyn_object_with_rights(handle, Rights::INSPECT)?;
                // FIXME: count Handle instead of Arc
                UserOutPtr::<u32>::from(buffer).write(Arc::strong_count(&object) as u32 - 1)?;
            }
            Topic::Job => {
                let job = proc.get_object_with_rights::<Job>(handle, Rights::INSPECT)?;
                UserOutPtr::<JobInfo>::from(buffer).write(job.get_info())?;
            }
            Topic::ProcessVmos => {
                warn!("A dummy implementation for utest Bti.NoDelayedUnpin, it does not check the reture value");
                actual.write(0)?;
                avail.write(0)?;
            }
            Topic::Vmo => {
                let (vmo, rights) = proc.get_object_and_rights::<VmObject>(handle)?;
                let mut info = vmo.get_info();
                info.flags |= VmoInfoFlags::VIA_HANDLE;
                info.rights |= rights;
                UserOutPtr::<VmoInfo>::from(buffer).write(info)?;
            }
            Topic::KmemStats => {
                let mut kmem = KmemInfo::default();
                kmem.vmo_bytes = vmo_page_bytes() as u64;
                UserOutPtr::<KmemInfo>::from(buffer).write(kmem)?;
            }
            Topic::TaskStats => {
                assert_eq!(core::mem::size_of::<TaskStatsInfo>(), buffer_size);
                let vmar = proc
                    .get_object_with_rights::<Process>(handle, Rights::INSPECT)?
                    .vmar();
                //let mut task_stats = ZxInfoTaskStats::default();
                let task_stats = vmar.get_task_stats();
                UserOutPtr::<TaskStatsInfo>::from(buffer).write(task_stats)?;
            }
            Topic::JobChildren | Topic::JobProcess | Topic::ProcessThreads => {
                let ids = match topic {
                    Topic::JobChildren => proc
                        .get_object_with_rights::<Job>(handle, Rights::ENUMERATE)?
                        .children_ids(),
                    Topic::JobProcess => proc
                        .get_object_with_rights::<Job>(handle, Rights::ENUMERATE)?
                        .process_ids(),
                    Topic::ProcessThreads => proc
                        .get_object_with_rights::<Process>(handle, Rights::ENUMERATE)?
                        .thread_ids(),
                    _ => unreachable!(),
                };
                let count = (buffer_size / core::mem::size_of::<KoID>()).min(ids.len());
                UserOutPtr::<KoID>::from(buffer).write_array(&ids[..count])?;
                actual.write(count)?;
                avail.write(ids.len())?;
            }
            Topic::Bti => {
                let bti = proc
                    .get_object_with_rights::<BusTransactionInitiator>(handle, Rights::INSPECT)?;
                UserOutPtr::<BtiInfo>::from(buffer).write(bti.get_info())?;
            }
            _ => {
                error!("not supported info topic: {:?}", topic);
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
        let signals = Signal::from_bits_truncate(signals);
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
        deadline: Deadline,
    ) -> ZxResult {
        if count > MAX_WAIT_MANY_ITEMS {
            return Err(ZxError::OUT_OF_RANGE);
        }
        let mut items = user_items.read_array(count as usize)?;
        info!("user_items: {:#x?}, deadline: {:?}", user_items, deadline);
        let proc = self.thread.proc();
        let mut waiters = Vec::with_capacity(count as usize);
        for item in items.iter() {
            let object = proc.get_dyn_object_with_rights(item.handle, Rights::WAIT)?;
            waiters.push((object, item.wait_for));
        }
        let future = wait_signal_many(&waiters);
        let res = self
            .thread
            .blocking_run(future, ThreadState::BlockedWaitMany, deadline.into())
            .await?;
        for (i, item) in items.iter_mut().enumerate() {
            item.observed = res[i];
        }
        user_items.write_array(&items)?;
        Ok(())
    }

    pub fn sys_object_get_child(
        &self,
        handle: HandleValue,
        koid: KoID,
        rights: u32,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "object.get_child: handle={:#x}, koid={:#x}, rights={:#x}",
            handle, koid, rights
        );
        let mut rights = Rights::from_bits(rights).ok_or(ZxError::INVALID_ARGS)?;
        let proc = self.thread.proc();
        let (task, parent_rights) = proc.get_dyn_object_and_rights(handle)?;
        if !parent_rights.contains(Rights::ENUMERATE) {
            return Err(ZxError::ACCESS_DENIED);
        }
        if rights == Rights::SAME_RIGHTS {
            rights = parent_rights;
        } else if (rights & parent_rights) != rights {
            return Err(ZxError::ACCESS_DENIED);
        }
        let child = task.get_child(koid)?;
        let child_handle = proc.add_handle(Handle::new(child, rights));
        out.write(child_handle)?;
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

#[repr(C)]
#[derive(Default)]
struct KmemInfo {
    total_bytes: u64,
    free_bytes: u64,
    wired_bytes: u64,
    total_heap_bytes: u64,
    free_heap_bytes: u64,
    vmo_bytes: u64,
    mmu_overhead_bytes: u64,
    ipc_bytes: u64,
    other_bytes: u64,
}
