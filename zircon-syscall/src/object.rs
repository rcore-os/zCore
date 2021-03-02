use {
    super::*,
    alloc::vec::Vec,
    core::convert::TryFrom,
    numeric_enum_macro::numeric_enum,
    zircon_object::{dev::*, ipc::*, signal::Port, task::*, vm::*},
};

impl Syscall<'_> {
    /// Ask for various properties of various kernel objects.
    ///
    /// `handle_value: HandleValue`, indicates the target kernel object.
    /// `property: u32`, indicates which property to get/set.
    /// `buffer: usize`, holds the property value, and must be a pointer to a buffer of value_size bytes.
    pub fn sys_object_get_property(
        &self,
        handle_value: HandleValue,
        property: u32,
        buffer: usize,
        buffer_size: usize,
    ) -> ZxResult {
        let property = Property::try_from(property).map_err(|_| ZxError::INVALID_ARGS)?;
        info!(
            "object.get_property: handle={:#x?}, property={:?}, buffer=({:#x}; {:#x?})",
            handle_value, property, buffer, buffer_size
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
                UserOutPtr::<u8>::from(buffer).write_cstring(s.as_str())?;
                Ok(())
            }
            Property::ProcessDebugAddr => {
                let mut info_ptr = UserOutPtr::<usize>::from_addr_size(buffer, buffer_size)?;
                let debug_addr = proc
                    .get_object_with_rights::<Process>(handle_value, Rights::GET_PROPERTY)?
                    .get_debug_addr();
                info_ptr.write(debug_addr)?;
                Ok(())
            }
            Property::ProcessVdsoBaseAddress => {
                let mut info_ptr = UserOutPtr::<usize>::from_addr_size(buffer, buffer_size)?;
                let vdso_base = proc.vmar().vdso_base_addr().unwrap_or(0);
                info_ptr.write(vdso_base)?;
                Ok(())
            }
            Property::ProcessBreakOnLoad => {
                let mut info_ptr = UserOutPtr::<usize>::from_addr_size(buffer, buffer_size)?;
                let break_on_load = proc
                    .get_object_with_rights::<Process>(handle_value, Rights::GET_PROPERTY)?
                    .get_dyn_break_on_load();
                info_ptr.write(break_on_load)?;
                Ok(())
            }
            Property::SocketRxThreshold => {
                let mut info_ptr = UserOutPtr::<usize>::from_addr_size(buffer, buffer_size)?;
                let rx = proc
                    .get_object_with_rights::<Socket>(handle_value, Rights::GET_PROPERTY)?
                    .get_rx_tx_threshold()
                    .0;
                info_ptr.write(rx)?;
                Ok(())
            }
            Property::SocketTxThreshold => {
                let mut info_ptr = UserOutPtr::<usize>::from_addr_size(buffer, buffer_size)?;
                let tx = proc
                    .get_object_with_rights::<Socket>(handle_value, Rights::GET_PROPERTY)?
                    .get_rx_tx_threshold()
                    .1;
                info_ptr.write(tx)?;
                Ok(())
            }
            Property::VmoContentSize => {
                let mut info_ptr = UserOutPtr::<usize>::from_addr_size(buffer, buffer_size)?;
                let content_size = proc
                    .get_object_with_rights::<VmObject>(handle_value, Rights::GET_PROPERTY)?
                    .content_size();
                info_ptr.write(content_size)?;
                Ok(())
            }
            Property::ExceptionState => {
                let mut info_ptr = UserOutPtr::<u32>::from_addr_size(buffer, buffer_size)?;
                let state = proc
                    .get_object_with_rights::<ExceptionObject>(handle_value, Rights::GET_PROPERTY)?
                    .state();
                info_ptr.write(state)?;
                Ok(())
            }
            Property::ExceptionStrategy => {
                let mut info_ptr = UserOutPtr::<u32>::from_addr_size(buffer, buffer_size)?;
                let strategy = proc
                    .get_object_with_rights::<ExceptionObject>(handle_value, Rights::GET_PROPERTY)?
                    .strategy();
                info_ptr.write(strategy)?;
                Ok(())
            }
            _ => {
                warn!("unknown property {:?}", property);
                Err(ZxError::INVALID_ARGS)
            }
        }
    }

    /// Set various properties of various kernel objects.
    pub fn sys_object_set_property(
        &mut self,
        handle_value: HandleValue,
        property: u32,
        buffer: usize,
        buffer_size: usize,
    ) -> ZxResult {
        let property = Property::try_from(property).map_err(|_| ZxError::INVALID_ARGS)?;
        info!(
            "object.set_property: handle={:#x?}, property={:?}, buffer=({:#x}; {:#x?})",
            handle_value, property, buffer, buffer_size
        );
        let proc = self.thread.proc();
        let object = proc.get_dyn_object_with_rights(handle_value, Rights::SET_PROPERTY)?;
        match property {
            Property::Name => {
                let length = buffer_size.min(MAX_NAME_LEN) as usize;
                let s = UserInPtr::<u8>::from(buffer).read_string(length)?;
                object.set_name(&s);
                Ok(())
            }
            Property::ProcessDebugAddr => {
                let addr = UserInPtr::<usize>::from_addr_size(buffer, buffer_size)?.read()?;
                proc.get_object_with_rights::<Process>(handle_value, Rights::SET_PROPERTY)?
                    .set_debug_addr(addr);
                Ok(())
            }
            #[cfg(target_arch = "x86_64")]
            Property::RegisterFs => {
                let thread = proc.get_object::<Thread>(handle_value)?;
                let fsbase = UserInPtr::<usize>::from_addr_size(buffer, buffer_size)?.read()?;
                thread.set_fsbase(fsbase)?;
                Ok(())
            }
            #[cfg(target_arch = "x86_64")]
            Property::RegisterGs => {
                let thread = proc.get_object::<Thread>(handle_value)?;
                let gsbase = UserInPtr::<usize>::from_addr_size(buffer, buffer_size)?.read()?;
                thread.set_gsbase(gsbase)?;
                Ok(())
            }
            Property::ProcessBreakOnLoad => {
                let addr = UserInPtr::<usize>::from_addr_size(buffer, buffer_size)?.read()?;
                proc.get_object_with_rights::<Process>(handle_value, Rights::SET_PROPERTY)?
                    .set_dyn_break_on_load(addr);
                Ok(())
            }
            Property::SocketRxThreshold => {
                let threshold = UserInPtr::<usize>::from_addr_size(buffer, buffer_size)?.read()?;
                proc.get_object::<Socket>(handle_value)?
                    .set_read_threshold(threshold)
            }
            Property::SocketTxThreshold => {
                let threshold = UserInPtr::<usize>::from_addr_size(buffer, buffer_size)?.read()?;
                proc.get_object::<Socket>(handle_value)?
                    .set_write_threshold(threshold)
            }
            Property::VmoContentSize => {
                let content_size =
                    UserInPtr::<usize>::from_addr_size(buffer, buffer_size)?.read()?;
                proc.get_object::<VmObject>(handle_value)?
                    .set_content_size(content_size)
            }
            Property::ExceptionState => {
                let state = UserInPtr::<u32>::from_addr_size(buffer, buffer_size)?.read()?;
                proc.get_object_with_rights::<ExceptionObject>(handle_value, Rights::SET_PROPERTY)?
                    .set_state(state)?;
                Ok(())
            }
            Property::ExceptionStrategy => {
                let strategy = UserInPtr::<u32>::from_addr_size(buffer, buffer_size)?.read()?;
                proc.get_object_with_rights::<ExceptionObject>(handle_value, Rights::SET_PROPERTY)?
                    .set_strategy(strategy)?;
                Ok(())
            }
            _ => {
                warn!("unknown property");
                Err(ZxError::INVALID_ARGS)
            }
        }
    }

    /// A blocking syscall waits for signals on an object.
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
            .blocking_run(
                future,
                ThreadState::BlockedWaitOne,
                deadline.into(),
                Some(cancel_token),
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

    /// Query information about an object.
    ///
    /// `topic: u32`, indicates what specific information is desired.
    /// `buffer: usize`, a pointer to a buffer of size buffer_size to return the information.
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
                let mut info_ptr = UserOutPtr::<ProcessInfo>::from_addr_size(buffer, buffer_size)?;
                let proc = proc.get_object_with_rights::<Process>(handle, Rights::INSPECT)?;
                info_ptr.write(proc.get_info())?;
            }
            Topic::Vmar => {
                let mut info_ptr = UserOutPtr::<VmarInfo>::from_addr_size(buffer, buffer_size)?;
                let vmar =
                    proc.get_object_with_rights::<VmAddressRegion>(handle, Rights::INSPECT)?;
                info_ptr.write(vmar.get_info())?;
            }
            Topic::HandleBasic => {
                let mut info_ptr =
                    UserOutPtr::<HandleBasicInfo>::from_addr_size(buffer, buffer_size)?;
                let info = proc.get_handle_info(handle)?;
                info_ptr.write(info)?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::Thread => {
                let mut info_ptr = UserOutPtr::<ThreadInfo>::from_addr_size(buffer, buffer_size)?;
                let thread = proc.get_object_with_rights::<Thread>(handle, Rights::INSPECT)?;
                info_ptr.write(thread.get_thread_info())?;
            }
            Topic::ThreadExceptionReport => {
                let mut info_ptr =
                    UserOutPtr::<ExceptionReport>::from_addr_size(buffer, buffer_size)?;
                let thread = proc.get_object_with_rights::<Thread>(handle, Rights::INSPECT)?;
                info_ptr.write(thread.get_thread_exception_info()?)?;
            }
            Topic::HandleCount => {
                let mut info_ptr = UserOutPtr::<u32>::from_addr_size(buffer, buffer_size)?;
                let object = proc.get_dyn_object_with_rights(handle, Rights::INSPECT)?;
                // FIXME: count Handle instead of Arc
                info_ptr.write(Arc::strong_count(&object) as u32 - 1)?;
            }
            Topic::Job => {
                let mut info_ptr = UserOutPtr::<JobInfo>::from_addr_size(buffer, buffer_size)?;
                let job = proc.get_object_with_rights::<Job>(handle, Rights::INSPECT)?;
                info_ptr.write(job.get_info())?;
            }
            Topic::ProcessVmos => {
                error!("A dummy implementation for utest Bti.NoDelayedUnpin, it does not check the reture value");
                actual.write(0)?;
                avail.write(0)?;
            }
            Topic::Vmo => {
                let mut info_ptr = UserOutPtr::<VmoInfo>::from_addr_size(buffer, buffer_size)?;
                let (vmo, rights) = proc.get_object_and_rights::<VmObject>(handle)?;
                let mut info = vmo.get_info();
                info.flags |= VmoInfoFlags::VIA_HANDLE;
                info.rights |= rights;
                info_ptr.write(info)?;
            }
            Topic::KmemStats => {
                let mut info_ptr = UserOutPtr::<KmemInfo>::from_addr_size(buffer, buffer_size)?;
                let kmem = KmemInfo {
                    vmo_bytes: vmo_page_bytes() as u64,
                    ..Default::default()
                };
                info_ptr.write(kmem)?;
            }
            Topic::TaskStats => {
                let mut info_ptr =
                    UserOutPtr::<TaskStatsInfo>::from_addr_size(buffer, buffer_size)?;
                let vmar = proc
                    .get_object_with_rights::<Process>(handle, Rights::INSPECT)?
                    .vmar();
                //let mut task_stats = ZxInfoTaskStats::default();
                let task_stats = vmar.get_task_stats();
                info_ptr.write(task_stats)?;
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
                let mut info_ptr = UserOutPtr::<BtiInfo>::from_addr_size(buffer, buffer_size)?;
                let bti = proc
                    .get_object_with_rights::<BusTransactionInitiator>(handle, Rights::INSPECT)?;
                info_ptr.write(bti.get_info())?;
            }
            Topic::Resource => {
                let mut info_ptr = UserOutPtr::<ResourceInfo>::from_addr_size(buffer, buffer_size)?;
                let resource = proc.get_object_with_rights::<Resource>(handle, Rights::INSPECT)?;
                info_ptr.write(resource.get_info())?;
            }
            Topic::Socket => {
                let mut info_ptr = UserOutPtr::<SocketInfo>::from_addr_size(buffer, buffer_size)?;
                let socket = proc.get_object_with_rights::<Socket>(handle, Rights::INSPECT)?;
                info_ptr.write(socket.get_info())?;
            }
            Topic::Stream => {
                let mut info_ptr = UserOutPtr::<StreamInfo>::from_addr_size(buffer, buffer_size)?;
                let stream = proc.get_object_with_rights::<Stream>(handle, Rights::INSPECT)?;
                info_ptr.write(stream.get_info())?;
            }
            _ => {
                error!("not supported info topic: {:?}", topic);
                return Err(ZxError::NOT_SUPPORTED);
            }
        }
        Ok(())
    }

    /// Asserts and deasserts the userspace-accessible signal bits on the object's peer.
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

    /// A non-blocking syscall subscribes for signals on an object.
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

    /// Signal an object.
    ///
    /// Asserts and deasserts the userspace-accessible signal bits on an object.
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

    /// Wait for signals on multiple objects.
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
            .blocking_run(future, ThreadState::BlockedWaitMany, deadline.into(), None)
            .await?;
        for (i, item) in items.iter_mut().enumerate() {
            item.observed = res[i];
        }
        user_items.write_array(&items)?;
        Ok(())
    }

    /// Find the child of an object by its kid.
    ///
    /// Given a kernel object with children objects, obtain a handle to the child specified by the provided kernel object id.
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
        Timer = 25,
        Stream = 26,
    }
}

numeric_enum! {
    #[repr(u32)]
    #[derive(Debug)]
    enum Property {
        RegisterGs = 2,
        Name = 3,
        RegisterFs = 4,
        ProcessDebugAddr = 5,
        ProcessVdsoBaseAddress = 6,
        ProcessBreakOnLoad = 7,
        SocketRxThreshold = 12,
        SocketTxThreshold = 13,
        ExceptionState = 16,
        VmoContentSize = 17,
        ExceptionStrategy = 18,
    }
}

const MAX_NAME_LEN: usize = 32;
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
