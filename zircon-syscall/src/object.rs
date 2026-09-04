use {
    super::*,
    alloc::vec::Vec,
    core::convert::TryFrom,
    numeric_enum_macro::numeric_enum,
    zircon_object::{
        dev::*,
        ipc::*,
        signal::{Port, Timer},
        task::*,
        vm::*,
    },
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
                let mut name = [0u8; MAX_NAME_LEN];
                let bytes = s.as_bytes();
                let len = bytes.len().min(MAX_NAME_LEN - 1);
                name[..len].copy_from_slice(&bytes[..len]);
                UserOutPtr::<u8>::from(buffer).write_array(&name)?;
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
            Property::StreamModeAppend => {
                let mut info_ptr = UserOutPtr::<u8>::from_addr_size(buffer, buffer_size)?;
                let append = proc
                    .get_object_with_rights::<Stream>(handle_value, Rights::GET_PROPERTY)?
                    .append_mode();
                info_ptr.write(append.into())?;
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
                let length = buffer_size.min(MAX_NAME_LEN);
                let name = UserInPtr::<u8>::from(buffer).as_str(length)?;
                object.set_name(name.split('\0').next().unwrap_or(""));
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
                thread.with_context(|ctx| ctx.general_mut().fsbase = fsbase)?;
                Ok(())
            }
            #[cfg(target_arch = "x86_64")]
            Property::RegisterGs => {
                let thread = proc.get_object::<Thread>(handle_value)?;
                let gsbase = UserInPtr::<usize>::from_addr_size(buffer, buffer_size)?.read()?;
                thread.with_context(|ctx| ctx.general_mut().gsbase = gsbase)?;
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
                proc.get_object_with_rights::<VmObject>(
                    handle_value,
                    Rights::WRITE | Rights::SET_PROPERTY,
                )?
                .set_content_size(content_size)
            }
            Property::StreamModeAppend => {
                let append = UserInPtr::<u8>::from_addr_size(buffer, buffer_size)?.read()?;
                proc.get_object_with_rights::<Stream>(handle_value, Rights::SET_PROPERTY)?
                    .set_append_mode(append != 0);
                Ok(())
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
                warn!("unknown property {:?}", property);
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
        if matches!(
            topic,
            Topic::ProcessV1
                | Topic::Process
                | Topic::Vmar
                | Topic::HandleBasic
                | Topic::Thread
                | Topic::ThreadStats
                | Topic::ThreadExceptionReportV1
                | Topic::ThreadExceptionReport
                | Topic::TaskRuntimeV1
                | Topic::TaskRuntime
                | Topic::HandleCount
                | Topic::Job
                | Topic::Timer
                | Topic::VmoV1
                | Topic::VmoV2
                | Topic::VmoV3
                | Topic::Vmo
                | Topic::KmemStatsV1
                | Topic::KmemStats
                | Topic::KmemStatsExtended
                | Topic::TaskStatsV1
                | Topic::TaskStats
                | Topic::Bti
                | Topic::Resource
                | Topic::Socket
                | Topic::Stream
                | Topic::ClockMappedSize
        ) {
            // Single-record topics report availability even when the caller's
            // output buffer is too small.
            actual.write_if_not_null(0)?;
            avail.write_if_not_null(1)?;
        }
        match topic {
            Topic::HandleValid => {
                let _ = proc.get_dyn_object_with_rights(handle, Rights::empty())?;
            }
            Topic::ProcessV1 | Topic::Process => {
                let target = proc.get_object_with_rights::<Process>(handle, Rights::INSPECT)?;
                let info = target.get_info();
                if topic == Topic::ProcessV1 {
                    let mut info_ptr =
                        UserOutPtr::<ProcessInfo>::from_addr_size(buffer, buffer_size)?;
                    info_ptr.write(info)?;
                } else {
                    let mut flags = 0;
                    if info.started {
                        flags |= PROCESS_FLAG_STARTED;
                    }
                    if info.has_exited {
                        flags |= PROCESS_FLAG_EXITED;
                    }
                    if info.debugger_attached {
                        flags |= PROCESS_FLAG_DEBUGGER_ATTACHED;
                    }
                    let mut info_ptr =
                        UserOutPtr::<ProcessInfoV2>::from_addr_size(buffer, buffer_size)?;
                    info_ptr.write(ProcessInfoV2 {
                        return_code: info.return_code,
                        // zCore does not currently retain a process start timestamp.
                        start_time: 0,
                        flags,
                        padding: [0; 4],
                    })?;
                }
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::Vmar => {
                let mut info_ptr = UserOutPtr::<VmarInfo>::from_addr_size(buffer, buffer_size)?;
                let vmar =
                    proc.get_object_with_rights::<VmAddressRegion>(handle, Rights::INSPECT)?;
                info_ptr.write(vmar.get_info())?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
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
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::ThreadStats => {
                let mut info_ptr =
                    UserOutPtr::<ThreadStatsInfo>::from_addr_size(buffer, buffer_size)?;
                let thread = proc.get_object_with_rights::<Thread>(handle, Rights::INSPECT)?;
                info_ptr.write(ThreadStatsInfo {
                    total_runtime: thread.get_time(),
                    last_scheduled_cpu: u32::MAX,
                    padding: [0; 4],
                })?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::ThreadExceptionReportV1 => {
                let mut info_ptr =
                    UserOutPtr::<ExceptionReportV1>::from_addr_size(buffer, buffer_size)?;
                let thread = proc.get_object_with_rights::<Thread>(handle, Rights::INSPECT)?;
                info_ptr.write(thread.get_thread_exception_info_v1()?)?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::ThreadExceptionReport => {
                let mut info_ptr =
                    UserOutPtr::<ExceptionReport>::from_addr_size(buffer, buffer_size)?;
                let thread = proc.get_object_with_rights::<Thread>(handle, Rights::INSPECT)?;
                info_ptr.write(thread.get_thread_exception_info()?)?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::TaskRuntimeV1 => {
                let thread = proc.get_object_with_rights::<Thread>(handle, Rights::INSPECT)?;
                let mut info_ptr =
                    UserOutPtr::<TaskRuntimeInfoV1>::from_addr_size(buffer, buffer_size)?;
                info_ptr.write(thread.get_runtime_info().into())?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::TaskRuntime => {
                let thread = proc.get_object_with_rights::<Thread>(handle, Rights::INSPECT)?;
                let mut info_ptr =
                    UserOutPtr::<TaskRuntimeInfo>::from_addr_size(buffer, buffer_size)?;
                info_ptr.write(thread.get_runtime_info())?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::HandleCount => {
                let mut info_ptr = UserOutPtr::<u32>::from_addr_size(buffer, buffer_size)?;
                let object = proc.get_dyn_object_with_rights(handle, Rights::INSPECT)?;
                // FIXME: count Handle instead of Arc
                info_ptr.write(Arc::strong_count(&object) as u32 - 1)?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::Job => {
                let mut info_ptr = UserOutPtr::<JobInfo>::from_addr_size(buffer, buffer_size)?;
                let job = proc.get_object_with_rights::<Job>(handle, Rights::INSPECT)?;
                info_ptr.write(job.get_info())?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::Timer => {
                let mut info_ptr = UserOutPtr::<TimerInfo>::from_addr_size(buffer, buffer_size)?;
                let timer = proc.get_object_with_rights::<Timer>(handle, Rights::INSPECT)?;
                let (options, deadline, slack) = timer.get_info();
                info_ptr.write(TimerInfo {
                    options,
                    clock_id: 0,
                    deadline,
                    slack,
                })?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::ProcessVmos => {
                warn!(
                    "A dummy implementation for utest Bti.NoDelayedUnpin, it does not check the reture value"
                );
                actual.write_if_not_null(0)?;
                avail.write_if_not_null(0)?;
            }
            Topic::VmoV1 | Topic::VmoV2 | Topic::VmoV3 | Topic::Vmo => {
                let (vmo, rights) = proc.get_object_and_rights::<VmObject>(handle)?;
                let mut info = vmo.get_info();
                info.flags |= VmoInfoFlags::VIA_HANDLE;
                info.rights |= rights;
                let v1 = VmoInfoV1::from(&info);
                match topic {
                    Topic::VmoV1 => {
                        UserOutPtr::<VmoInfoV1>::from_addr_size(buffer, buffer_size)?.write(v1)?;
                    }
                    Topic::VmoV2 => {
                        UserOutPtr::<VmoInfoV2>::from_addr_size(buffer, buffer_size)?.write(
                            VmoInfoV2 {
                                v1,
                                metadata_bytes: info.metadata_bytes,
                                committed_change_events: info.committed_change_events,
                            },
                        )?;
                    }
                    Topic::VmoV3 => {
                        UserOutPtr::<VmoInfoV3>::from_addr_size(buffer, buffer_size)?.write(
                            VmoInfoV3 {
                                v2: VmoInfoV2 {
                                    v1,
                                    metadata_bytes: info.metadata_bytes,
                                    committed_change_events: info.committed_change_events,
                                },
                                populated_bytes: info.populated_bytes,
                            },
                        )?;
                    }
                    Topic::Vmo => {
                        UserOutPtr::<VmoInfo>::from_addr_size(buffer, buffer_size)?.write(info)?;
                    }
                    _ => unreachable!(),
                }
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::KmemStatsV1 | Topic::KmemStats | Topic::KmemStatsExtended => {
                let _resource = proc.get_object_with_rights::<Resource>(handle, Rights::INSPECT)?;
                let vmo_bytes = vmo_page_bytes() as u64;
                if topic == Topic::KmemStatsV1 {
                    let mut info_ptr =
                        UserOutPtr::<KmemInfoV1>::from_addr_size(buffer, buffer_size)?;
                    info_ptr.write(KmemInfoV1 {
                        vmo_bytes,
                        ..Default::default()
                    })?;
                } else if topic == Topic::KmemStatsExtended {
                    let mut info_ptr =
                        UserOutPtr::<KmemInfoExtended>::from_addr_size(buffer, buffer_size)?;
                    info_ptr.write(KmemInfoExtended {
                        vmo_bytes,
                        ..Default::default()
                    })?;
                } else {
                    let mut info_ptr = UserOutPtr::<KmemInfo>::from_addr_size(buffer, buffer_size)?;
                    info_ptr.write(KmemInfo {
                        vmo_bytes,
                        ..Default::default()
                    })?;
                }
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::TaskStatsV1 | Topic::TaskStats => {
                let vmar = proc
                    .get_object_with_rights::<Process>(handle, Rights::INSPECT)?
                    .vmar();
                let task_stats = vmar.get_task_stats();
                if topic == Topic::TaskStatsV1 {
                    let mut info_ptr =
                        UserOutPtr::<TaskStatsInfoV1>::from_addr_size(buffer, buffer_size)?;
                    info_ptr.write(TaskStatsInfoV1 {
                        mapped_bytes: task_stats.mapped_bytes,
                        private_bytes: task_stats.private_bytes,
                        shared_bytes: task_stats.shared_bytes,
                        scaled_shared_bytes: task_stats.scaled_shared_bytes,
                    })?;
                } else {
                    let mut info_ptr =
                        UserOutPtr::<TaskStatsInfo>::from_addr_size(buffer, buffer_size)?;
                    info_ptr.write(task_stats)?;
                }
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
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
                crate::channel::validate_user_range(
                    proc,
                    buffer,
                    count * core::mem::size_of::<KoID>(),
                    MMUFlags::WRITE,
                )?;
                UserOutPtr::<KoID>::from(buffer).write_array(&ids[..count])?;
                actual.write_if_not_null(count)?;
                avail.write_if_not_null(ids.len())?;
            }
            Topic::Bti => {
                let mut info_ptr = UserOutPtr::<BtiInfo>::from_addr_size(buffer, buffer_size)?;
                let bti = proc
                    .get_object_with_rights::<BusTransactionInitiator>(handle, Rights::INSPECT)?;
                info_ptr.write(bti.get_info())?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::Resource => {
                let mut info_ptr = UserOutPtr::<ResourceInfo>::from_addr_size(buffer, buffer_size)?;
                let resource = proc.get_object_with_rights::<Resource>(handle, Rights::INSPECT)?;
                info_ptr.write(resource.get_info())?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::Socket => {
                let mut info_ptr = UserOutPtr::<SocketInfo>::from_addr_size(buffer, buffer_size)?;
                let socket = proc.get_object_with_rights::<Socket>(handle, Rights::INSPECT)?;
                info_ptr.write(socket.get_info())?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::Stream => {
                let mut info_ptr = UserOutPtr::<StreamInfo>::from_addr_size(buffer, buffer_size)?;
                let stream = proc.get_object_with_rights::<Stream>(handle, Rights::INSPECT)?;
                info_ptr.write(stream.get_info())?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
            }
            Topic::ClockMappedSize => {
                let mut info_ptr = UserOutPtr::<u64>::from_addr_size(buffer, buffer_size)?;
                let clock = proc.get_object_with_rights::<Clock>(handle, Rights::INSPECT)?;
                if clock.mapped_vmo().is_none() {
                    return Err(ZxError::INVALID_ARGS);
                }
                info_ptr.write(clock.mapped_size() as u64)?;
                actual.write_if_not_null(1)?;
                avail.write_if_not_null(1)?;
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
            return Err(ZxError::INVALID_ARGS);
        }
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
    #[derive(Debug, Eq, PartialEq)]
    enum Topic {
        None = 0,
        HandleValid = 1,
        HandleBasic = 2,
        ProcessV1 = 3,
        Process = 0x1000_0003,
        ProcessThreads = 4,
        Vmar = 7,
        JobChildren = 8,
        JobProcess = 9,
        Thread = 10,
        ThreadExceptionReportV1 = 11,
        ThreadExceptionReport = 0x1000_000b,
        TaskStatsV1 = 12,
        TaskStats = 0x1000_000c,
        ProcessMaps = 13,
        ProcessVmos = 14,
        ThreadStats = 15,
        CpuStats = 16,
        KmemStatsV1 = 17,
        KmemStats = 0x1000_0011,
        Resource = 18,
        HandleCount = 19,
        Bti = 20,
        ProcessHandleStats = 21,
        Socket = 22,
        VmoV1 = 23,
        VmoV2 = 0x1000_0017,
        VmoV3 = 0x2000_0017,
        Vmo = 0x3000_0017,
        Job = 24,
        Timer = 25,
        Stream = 26,
        KmemStatsExtended = 31,
        ClockMappedSize = 40,
        TaskRuntimeV1 = 30,
        TaskRuntime = 0x1000_001e,
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
        StreamModeAppend = 19,
    }
}

const MAX_NAME_LEN: usize = 32;
const MAX_WAIT_MANY_ITEMS: u32 = 32;
const PROCESS_FLAG_STARTED: u32 = 1 << 0;
const PROCESS_FLAG_EXITED: u32 = 1 << 1;
const PROCESS_FLAG_DEBUGGER_ATTACHED: u32 = 1 << 2;

#[repr(C)]
struct ProcessInfoV2 {
    return_code: i64,
    start_time: i64,
    flags: u32,
    padding: [u8; 4],
}

#[repr(C)]
struct ThreadStatsInfo {
    total_runtime: u64,
    last_scheduled_cpu: u32,
    padding: [u8; 4],
}

#[repr(C)]
struct TimerInfo {
    options: u32,
    clock_id: u32,
    deadline: u64,
    slack: u64,
}

#[repr(C)]
struct TaskStatsInfoV1 {
    mapped_bytes: u64,
    private_bytes: u64,
    shared_bytes: u64,
    scaled_shared_bytes: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VmoInfoV1 {
    koid: KoID,
    name: [u8; 32],
    size: u64,
    parent_koid: KoID,
    num_children: u64,
    num_mappings: u64,
    share_count: u64,
    flags: VmoInfoFlags,
    padding1: [u8; 4],
    committed_bytes: u64,
    rights: Rights,
    cache_policy: u32,
}

impl From<&VmoInfo> for VmoInfoV1 {
    fn from(info: &VmoInfo) -> Self {
        Self {
            koid: info.koid,
            name: info.name,
            size: info.size,
            parent_koid: info.parent_koid,
            num_children: info.num_children,
            num_mappings: info.num_mappings,
            share_count: info.share_count,
            flags: info.flags,
            padding1: info.padding1,
            committed_bytes: info.committed_bytes,
            rights: info.rights,
            cache_policy: info.cache_policy,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VmoInfoV2 {
    v1: VmoInfoV1,
    metadata_bytes: u64,
    committed_change_events: u64,
}

#[repr(C)]
struct VmoInfoV3 {
    v2: VmoInfoV2,
    populated_bytes: u64,
}

#[derive(Debug)]
#[repr(C)]
pub struct UserWaitItem {
    handle: HandleValue,
    wait_for: Signal,
    observed: Signal,
}

#[repr(C)]
#[derive(Default)]
struct KmemInfoV1 {
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

#[repr(C)]
#[derive(Default)]
struct KmemInfo {
    total_bytes: u64,
    free_bytes: u64,
    free_loaned_bytes: u64,
    wired_bytes: u64,
    total_heap_bytes: u64,
    free_heap_bytes: u64,
    vmo_bytes: u64,
    mmu_overhead_bytes: u64,
    ipc_bytes: u64,
    cache_bytes: u64,
    slab_bytes: u64,
    zram_bytes: u64,
    other_bytes: u64,
    vmo_reclaim_total_bytes: u64,
    vmo_reclaim_newest_bytes: u64,
    vmo_reclaim_oldest_bytes: u64,
    vmo_reclaim_disabled_bytes: u64,
    vmo_discardable_locked_bytes: u64,
    vmo_discardable_unlocked_bytes: u64,
}

#[repr(C)]
#[derive(Default)]
struct KmemInfoExtended {
    total_bytes: u64,
    free_bytes: u64,
    wired_bytes: u64,
    total_heap_bytes: u64,
    free_heap_bytes: u64,
    vmo_bytes: u64,
    vmo_pager_total_bytes: u64,
    vmo_pager_newest_bytes: u64,
    vmo_pager_oldest_bytes: u64,
    vmo_discardable_locked_bytes: u64,
    vmo_discardable_unlocked_bytes: u64,
    mmu_overhead_bytes: u64,
    ipc_bytes: u64,
    other_bytes: u64,
    vmo_reclaim_disabled_bytes: u64,
}
