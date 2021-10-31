//! Linux LibOS
//! - run process and manage trap/interrupt/syscall
#![no_std]
#![feature(asm)]
// #![deny(warnings, unused_must_use, missing_docs)]
#![deny(unused_must_use, missing_docs)]

extern crate alloc;
#[macro_use]
extern crate log;

use {
    alloc::{boxed::Box, string::String, sync::Arc, vec::Vec},
    core::{future::Future, pin::Pin},
    kernel_hal::MMUFlags,
    linux_object::{
        fs::{vfs::FileSystem, INodeExt},
        loader::LinuxElfLoader,
        process::ProcessExt,
        thread::ThreadExt,
    },
    linux_syscall::Syscall,
    zircon_object::task::*,
};

#[cfg(target_arch = "riscv64")]
use {kernel_hal::UserContext, zircon_object::object::KernelObject};

#[cfg(target_arch = "x86_64")]
use kernel_hal::GeneralRegs;

/// Create and run main Linux process
pub fn run(args: Vec<String>, envs: Vec<String>, rootfs: Arc<dyn FileSystem>) -> Arc<Process> {
    let job = Job::root();
    let proc = Process::create_linux(&job, rootfs.clone()).unwrap();
    let thread = Thread::create_linux(&proc).unwrap();
    let loader = LinuxElfLoader {
        #[cfg(feature = "std")]
        syscall_entry: kernel_hal_unix::syscall_entry as usize,
        #[cfg(not(feature = "std"))]
        syscall_entry: 0,
        stack_pages: 8,
        root_inode: rootfs.root_inode(),
    };

    {
        let mut id = 0;
        let rust_dir = rootfs.root_inode().lookup("/").unwrap();
        trace!("run(), Rootfs: / ");
        while let Ok(name) = rust_dir.get_entry(id) {
            id += 1;
            trace!("  {}", name);
        }
    }
    info!("args {:?}", args);
    let inode = rootfs.root_inode().lookup(&args[0]).unwrap();
    let data = inode.read_as_vec().unwrap();
    let path = args[0].clone();
    debug!("Linux process: {:?}", path);

    let pg_token = kernel_hal::current_page_table();
    debug!("current pgt = {:#x}", pg_token);
    //调用zircon-object/src/task/thread.start设置好要执行的thread
    let (entry, sp) = loader.load(&proc.vmar(), &data, args, envs, path).unwrap();

    thread
        .start(entry, sp, 0, 0, thread_fn)
        .expect("failed to start main thread");
    proc
}

//待实际测试是否可用？
/// Create and run a Linux process
pub fn run_linux_proc(args: Vec<String>, entry: usize) -> Arc<Process> {
    use rcore_fs_ramfs::RamFS;
    let rootfs = RamFS::new();

    let job = Job::root();
    let proc = Process::create_linux(&job, rootfs.clone()).unwrap();
    let thread = Thread::create_linux(&proc).unwrap();

    info!("args {:?}", args);
    {
        let mut id = 0;
        let rust_dir = rootfs.root_inode().lookup("/").unwrap();
        debug!("Rootfs: / ");
        while let Ok(name) = rust_dir.get_entry(id) {
            id += 1;
            debug!("    {}", name);
        }
    }

    use zircon_object::vm::VmObject;
    let stack_vmo = VmObject::new_paged(8);
    let flags = MMUFlags::READ | MMUFlags::WRITE | MMUFlags::USER;
    let stack_bottom = proc
        .vmar()
        .map(None, stack_vmo.clone(), 0, stack_vmo.len(), flags)
        .unwrap();
    //let sp = stack_bottom + stack_vmo.len();
    let sp = stack_bottom + stack_vmo.len() - 4096;
    debug!("load stack bottom: {:#x} -- {:#x}", stack_bottom, sp);

    thread
        .start(entry, sp, 0, 0, thread_fn)
        .expect("failed to start main thread");
    proc
}

/// The function of a new thread.
///
/// loop:
/// - wait for the thread to be ready
/// - get user thread context
/// - enter user mode
/// - handle trap/interrupt/syscall according to the return value
/// - return the context to the user thread
async fn new_thread(thread: CurrentThread) {
    loop {
        // wait
        let mut cx = thread.wait_for_run().await;
        if thread.state() == ThreadState::Dying {
            break;
        }

        //========= 网络认证 添加 区域 start =======
        // super_mode_net_udp_server_test();
        // super_mode_net_tcp_server_test();
        // super_mode_net_tcp_client_test();
        // super_mode_frame_test();

        // use core::time::Duration;
        // use kernel_hal::sleep_until;
        // use kernel_hal::timer_now;
        // ping().await;
        // super_mode_eap_test().await;
        // ping().await;

        // loop {}

        //========= 网络认证 添加 区域  end =======

        // run
        trace!("go to user: {:#x?}", cx);
        kernel_hal::context_run(&mut cx);
        trace!("back from user: {:#x?}", cx);
        // handle trap/interrupt/syscall

        #[cfg(target_arch = "x86_64")]
        match cx.trap_num {
            0x100 => handle_syscall(&thread, &mut cx.general).await,
            0x20..=0x3f => {
                kernel_hal::InterruptManager::handle(cx.trap_num as u8);
                if cx.trap_num == 0x20 {
                    kernel_hal::yield_now().await;
                }
            }
            0xe => {
                let vaddr = kernel_hal::fetch_fault_vaddr();
                let flags = if cx.error_code & 0x2 == 0 {
                    MMUFlags::READ
                } else {
                    MMUFlags::WRITE
                };
                error!("page fualt from user mode {:#x} {:#x?}", vaddr, flags);
                let vmar = thread.proc().vmar();
                match vmar.handle_page_fault(vaddr, flags) {
                    Ok(()) => {}
                    Err(_) => {
                        panic!("Page Fault from user mode {:#x?}", cx);
                    }
                }
            }
            _ => panic!("not supported interrupt from user mode. {:#x?}", cx),
        }

        // UserContext
        #[cfg(target_arch = "riscv64")]
        {
            let trap_num = kernel_hal::fetch_trap_num(&cx);
            let is_interrupt = ((trap_num >> 63) & 1) == 1;
            let trap_num = trap_num & 0xfff;
            let pid = thread.proc().id();
            if is_interrupt {
                match trap_num {
                    //Irq
                    0 | 4 | 5 | 8 | 9 => {
                        kernel_hal::InterruptManager::handle(trap_num as u8);

                        //Timer
                        if trap_num == 4 || trap_num == 5 {
                            debug!("Timer interrupt: {}", trap_num);

                            /*
                            * 已在irq_handle里加入了timer处理函数
                            kernel_hal::timer_set_next();
                            kernel_hal::timer_tick();
                            */

                            kernel_hal::yield_now().await;
                        }

                        //kernel_hal::InterruptManager::handle(trap_num as u8);
                    }
                    _ => panic!(
                        "not supported pid: {} interrupt {} from user mode. {:#x?}",
                        pid, trap_num, cx
                    ),
                }
            } else {
                match trap_num {
                    // syscall
                    8 => handle_syscall(&thread, &mut cx).await,
                    // PageFault
                    12 | 13 | 15 => {
                        let vaddr = kernel_hal::fetch_fault_vaddr();

                        //注意这里flags没有包含WRITE权限，后面handle会移除写权限
                        let flags = if trap_num == 15 {
                            MMUFlags::WRITE
                        } else if trap_num == 12 {
                            MMUFlags::EXECUTE
                        } else {
                            MMUFlags::READ
                        };

                        info!(
                            "page fualt from pid: {} user mode, vaddr:{:#x}, trap:{}",
                            pid, vaddr, trap_num
                        );
                        let vmar = thread.proc().vmar();
                        match vmar.handle_page_fault(vaddr, flags) {
                            Ok(()) => {}
                            Err(error) => {
                                panic!("{:?} Page Fault from user mode {:#x?}", error, cx);
                            }
                        }
                    }
                    _ => panic!(
                        "not supported pid: {} exception {} from user mode. {:#x?}",
                        pid, trap_num, cx
                    ),
                }
            }
        }
        thread.end_running(cx);
    }
}

fn thread_fn(thread: CurrentThread) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
    Box::pin(new_thread(thread))
}

/// syscall handler entry
#[cfg(target_arch = "x86_64")]
async fn handle_syscall(thread: &CurrentThread, regs: &mut GeneralRegs) {
    trace!("syscall: {:#x?}", regs);
    let num = regs.rax as u32;
    let args = [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9];
    let mut syscall = Syscall {
        thread,
        #[cfg(feature = "std")]
        syscall_entry: kernel_hal_unix::syscall_entry as usize,
        #[cfg(not(feature = "std"))]
        syscall_entry: 0,
        thread_fn,
        regs,
    };
    regs.rax = syscall.syscall(num, args).await as usize;
}

#[cfg(target_arch = "riscv64")]
async fn handle_syscall(thread: &CurrentThread, cx: &mut UserContext) {
    trace!("syscall: {:#x?}", cx.general);
    let num = cx.general.a7 as u32;
    let args = [
        cx.general.a0,
        cx.general.a1,
        cx.general.a2,
        cx.general.a3,
        cx.general.a4,
        cx.general.a5,
    ];
    // add before fork
    cx.sepc += 4;

    //注意, 此时的regs没有原context所有权，故无法通过此regs修改寄存器
    //let regs = &mut (cx.general as GeneralRegs);

    let mut syscall = Syscall {
        thread,
        #[cfg(feature = "std")]
        syscall_entry: kernel_hal_unix::syscall_entry as usize,
        #[cfg(not(feature = "std"))]
        syscall_entry: 0,
        context: cx,
        thread_fn,
    };
    cx.general.a0 = syscall.syscall(num, args).await as usize;
}

/// network testing
pub fn net_start_thread() {
    // let ping_before_auth_future = Box::pin(ping_test());
    let eap_future = Box::pin(eap_test());
    // let ping_after_auth_future = Box::pin(ping_test());
    let vmtoken = kernel_hal::current_page_table();
    // kernel_hal::Thread::spawn(ping_before_auth_future, vmtoken);
    kernel_hal::Thread::spawn(eap_future, vmtoken);
    // kernel_hal::Thread::spawn(ping_after_auth_future, vmtoken);
}

use kernel_hal::yield_now;
async fn eap_test() {
    // for n in 0..3 {
    ping().await;
    super_mode_eap_test().await;
    ping().await;
    // }
}

async fn ping_test() {
    ping().await;
}

#[allow(dead_code)]
#[allow(unused_imports)]
#[allow(unused_variables)]
async fn super_mode_eap_test() {
    use core::str::from_utf8;
    use kernel_hal::drivers::get_net_driver;
    use kernel_hal::timer_now;
    use kernel_hal_bare::drivers::net::rtl8x::RTL8xInterface;
    use smoltcp::socket::SocketSet;
    use smoltcp::socket::UdpPacketMetadata;
    use smoltcp::socket::UdpSocket;
    use smoltcp::socket::UdpSocketBuffer;
    use smoltcp::time::Instant;

    use smoltcp::wire::*;

    use core::time::Duration;
    use kernel_hal::sleep_until;

    use kernel_hal::timer_tick;

    let latency: u64 = 100; // 毫秒 millis

    let rtl8x = get_net_driver()[0].clone();

    if let Ok(li) = rtl8x.downcast_arc::<RTL8xInterface>() {
        // loop {
        let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
        // warn!("timestamp0 : {:?}",timestamp);
        sleep_until(timer_now() + Duration::from_millis(latency)).await;
        let _x = li.iface.lock().eapol(timestamp);
        warn!("<-- EAPOL Start from smoltcp");

        // sleep_until(timer_now() + Duration::from_secs(latency)).await;
        sleep_until(timer_now() + Duration::from_millis(latency)).await;

        // eap 1
        {
            let eapol_repr = EAPoLRepr::EthernetEAPoL {
                protocol_version: EAPoLProtocalVersion::X2001,
                eap_type: EAPoLType::Packet,
                len: 7,
            };

            let eap_repr = EAPRepr::EthernetEAP {
                code: EAPCode::Response,
                identifier: 1,
                len: 7,
            };

            let eap_data_repr = EAPDataRepr::EthernetEAPData {
                eapdata_type: EAPDataType::Identifier,
            };
            let f = |mut frame: EthernetFrame<&mut [u8]>| {
                // frame.set_dst_addr(mac_addr);
                frame.set_dst_addr(EthernetAddress::BROADCAST);
                frame.set_ethertype(EthernetProtocol::EAPoL);

                let mut eapol_packet = EAPoLPacket::new_unchecked(frame.payload_mut());
                eapol_repr.emit(&mut eapol_packet);
                let mut eap_packet = EAPPacket::new_unchecked(eapol_packet.packet_mut());
                eap_repr.emit(&mut eap_packet);
                let mut eap_data = EAPDataPacket::new_unchecked(eap_packet.data_mut());
                eap_data_repr.emit(&mut eap_data);

                // let typedata: [u8; 37] = [0x63, 0x6a,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00];
                let typedata: [u8; 2] = [0x63, 0x6a];
                eap_data.typedata_mut().copy_from_slice(&typedata[..]);

                // warn!("eframe : {:X?}", frame);
            };

            let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
            warn!("{:?} : EAP Respone  ", timestamp);
            let _x = li.iface.lock().eap(timestamp, eapol_repr.buffer_len(), f);
        }

        sleep_until(timer_now() + Duration::from_millis(latency)).await;

        // eap 2
        {
            let eapol_repr = EAPoLRepr::EthernetEAPoL {
                protocol_version: EAPoLProtocalVersion::X2001,
                eap_type: EAPoLType::Packet,
                len: 181,
            };

            let eap_repr = EAPRepr::EthernetEAP {
                code: EAPCode::Response,
                identifier: 2,
                len: 181,
            };

            let eap_data_repr = EAPDataRepr::EthernetEAPData {
                eapdata_type: EAPDataType::UnknownType,
            };
            let f = |mut frame: EthernetFrame<&mut [u8]>| {
                // frame.set_dst_addr(mac_addr);
                frame.set_dst_addr(EthernetAddress::BROADCAST);
                frame.set_ethertype(EthernetProtocol::EAPoL);

                let mut eapol_packet = EAPoLPacket::new_unchecked(frame.payload_mut());
                eapol_repr.emit(&mut eapol_packet);
                let mut eap_packet = EAPPacket::new_unchecked(eapol_packet.packet_mut());
                eap_repr.emit(&mut eap_packet);
                let mut eap_data = EAPDataPacket::new_unchecked(eap_packet.data_mut());
                eap_data_repr.emit(&mut eap_data);

                // let typedata: [u8; 15] = [0x43 ,0x68 ,0x20 ,0x20 ,0x20 ,0x58 ,0x69 ,0x6E ,0x67 ,0x20 ,0x7A ,0x43 ,0x6F ,0x72 ,0x65];
                // eap_data.typedata_mut().copy_from_slice(&typedata[..]);
                let typedata: [u8; 176] = [
                    0x0d, 0, 0, 0, 0x01, 0, 0, 0, 0x82, 0, 0, 0xc0, 0x58, 0x2b, 0x9d, 0xbe, 0x36,
                    0x5d, 0x69, 0x64, 0x04, 0x96, 0x57, 0xb5, 0x39, 0xa6, 0xfc, 0x73, 0x6f, 0x01,
                    0xc4, 0xe0, 0xb4, 0x51, 0x25, 0x27, 0x8c, 0x93, 0x6e, 0xef, 0xc9, 0x80, 0x30,
                    0x07, 0x0e, 0, 0x0d, 0, 0x58, 0x20, 0xc0, 0x58, 0x2b, 0x9d, 0xbe, 0x36, 0x5d,
                    0x69, 0x64, 0x04, 0x96, 0x57, 0xb5, 0x39, 0xa6, 0xfc, 0x73, 0x6f, 0x01, 0xc4,
                    0xe0, 0xb4, 0x51, 0x25, 0x27, 0x8c, 0x93, 0x6e, 0xef, 0xc9, 0x80, 0x30, 0x30,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x25, 0, 0,
                ];

                // 0000   01 80 c2 00 00 03 00 e0 4c 68 05 2c 88 8e 01 00   ........Lh.,....
                // 0010   00 b1 02 c7 00 b1 39 0d 00 00 00 82 00 00 c0 58   ......9........X
                // 0020   2b 9d be 36 5d 69 64 04 96 57 b5 39 a6 fc 73 6f   +..6]id..W.9..so
                // 0030   01 c4 e0 b4 51 25 27 8c 93 6e ef c9 80 30 07 00   ....Q%'..n...0..
                // 0040   0d 00 58 20 c0 58 2b 9d be 36 5d 69 64 04 96 57   ..X .X+..6]id..W
                // 0050   b5 39 a6 fc 73 6f 01 c4 e0 b4 51 25 27 8c 93 6e   .9..so....Q%'..n
                // 0060   ef c9 80 30 30 00 00 00 00 00 00 00 00 00 00 00   ...00...........
                // 0070   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0080   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0090   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 00a0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 00b0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 25   ...............%
                // 00c0   00 00 00                                          ...
                eap_data.typedata_mut().copy_from_slice(&typedata[..]);
                // warn!("eframe : {:X?}", frame);
            };

            let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
            warn!("{:?} : EAP Respone  ", timestamp);
            let _x = li.iface.lock().eap(timestamp, eapol_repr.buffer_len(), f);
        }

        sleep_until(timer_now() + Duration::from_millis(latency)).await;

        // eap 3
        {
            let eapol_repr = EAPoLRepr::EthernetEAPoL {
                protocol_version: EAPoLProtocalVersion::X2001,
                eap_type: EAPoLType::Packet,
                len: 827,
            };

            let eap_repr = EAPRepr::EthernetEAP {
                code: EAPCode::Response,
                identifier: 3,
                len: 827,
            };

            let eap_data_repr = EAPDataRepr::EthernetEAPData {
                eapdata_type: EAPDataType::UnknownType,
            };
            let f = |mut frame: EthernetFrame<&mut [u8]>| {
                // frame.set_dst_addr(mac_addr);
                frame.set_dst_addr(EthernetAddress::BROADCAST);
                frame.set_ethertype(EthernetProtocol::EAPoL);

                let mut eapol_packet = EAPoLPacket::new_unchecked(frame.payload_mut());
                eapol_repr.emit(&mut eapol_packet);
                let mut eap_packet = EAPPacket::new_unchecked(eapol_packet.packet_mut());
                eap_repr.emit(&mut eap_packet);
                let mut eap_data = EAPDataPacket::new_unchecked(eap_packet.data_mut());
                eap_data_repr.emit(&mut eap_data);

                let typedata: [u8; 822] = [
                    0x29, 0x58, 0x1e, 0xf5, 0x6b, 0xe7, 0xdb, 0xc3, 0x5a, 0xa7, 0x76, 0x47, 0x37,
                    0x12, 0xe3, 0x5b, 0x3c, 0x0a, 0xc0, 0x7f, 0x61, 0x7a, 0x3c, 0x75, 0x31, 0x10,
                    0xaf, 0xa9, 0x97, 0x90, 0xf8, 0xe4, 0xa8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0x21, 0xf5, 0x6b, 0xe7, 0xdb, 0xc3, 0x5a, 0xa7, 0x76, 0x47, 0x37,
                    0x12, 0xe3, 0x5b, 0x3c, 0x0a, 0xc0, 0x7f, 0x61, 0x7a, 0x3c, 0x75, 0x31, 0x10,
                    0xaf, 0xa9, 0x97, 0x90, 0xf8, 0xe4, 0xa8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0x1e, 0xeb, 0x6a, 0x0f, 0x36, 0xca, 0xb8, 0x6e, 0x18,
                    0xf1, 0x98, 0xd5, 0xf4, 0x60, 0xa0, 0x22, 0xbc, 0xe6, 0x36, 0x34, 0xf5, 0x30,
                    0xe9, 0x26, 0xea, 0x6a, 0xb6, 0x29, 0xb1, 0xda, 0x08, 0xca, 0xd0, 0xa4, 0x01,
                    0x01, 0x20, 0x04, 0x21, 0x58, 0x20, 0xc0, 0x58, 0x2b, 0x9d, 0xbe, 0x36, 0x5d,
                    0x69, 0x64, 0x04, 0x96, 0x57, 0xb5, 0x39, 0xa6, 0xfc, 0x73, 0x6f, 0x01, 0xc4,
                    0xe0, 0xb4, 0x51, 0x25, 0x27, 0x8c, 0x93, 0x6e, 0xef, 0xc9, 0x80, 0x30, 0x6c,
                    0x73, 0x75, 0x62, 0x6a, 0x65, 0x63, 0x74, 0x20, 0x6e, 0x61, 0x6d, 0x65, 0x60,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x36, 0xa1,
                    0x04, 0x41, 0x17, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x04, 0x31, 0xe2, 0x9a,
                    0xa2, 0x0c, 0x17, 0xca, 0xe1, 0xce, 0x9a, 0xf0, 0x87, 0x18, 0x0e, 0x8d, 0x55,
                    0x56, 0xd8, 0x4e, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0x0b, 0x83, 0x68, 0x45, 0x6e, 0x63, 0x72, 0x79, 0x70, 0x74, 0x30, 0x44, 0xa1,
                    0x04, 0x41, 0x17, 0x58, 0x58, 0x58, 0x20, 0xeb, 0x6a, 0x0f, 0x36, 0xca, 0xb8,
                    0x6e, 0x18, 0xf1, 0x98, 0xd5, 0xf4, 0x60, 0xa0, 0x22, 0xbc, 0xe6, 0x36, 0x34,
                    0xf5, 0x30, 0xe9, 0x26, 0xea, 0x6a, 0xb6, 0x29, 0xb1, 0xda, 0x08, 0xca, 0xd0,
                    0xa4, 0x01, 0x01, 0x20, 0x04, 0x21, 0x58, 0x20, 0xc0, 0x58, 0x2b, 0x9d, 0xbe,
                    0x36, 0x5d, 0x69, 0x64, 0x04, 0x96, 0x57, 0xb5, 0x39, 0xa6, 0xfc, 0x73, 0x6f,
                    0x01, 0xc4, 0xe0, 0xb4, 0x51, 0x25, 0x27, 0x8c, 0x93, 0x6e, 0xef, 0xc9, 0x80,
                    0x30, 0x6c, 0x73, 0x75, 0x62, 0x6a, 0x65, 0x63, 0x74, 0x20, 0x6e, 0x61, 0x6d,
                    0x65, 0x60, 0x4b, 0xce, 0x9a, 0xf0, 0x87, 0x18, 0x0e, 0x8d, 0x55, 0x56, 0xd8,
                    0x4e, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0x75,
                ];
                eap_data.typedata_mut().copy_from_slice(&typedata[..]);
                // 0000   01 80 c2 00 00 03 00 e0 4c 68 05 2c 88 8e 01 00   ........Lh.,....
                // 0010   03 3b 02 c8 03 3b 39 33 58 1d 99 6e c2 9c 8e 6d   .;...;93X..n...m
                // 0020   d9 85 b5 7f 03 38 61 9f 9d c2 38 d5 44 6f 26 4e   .....8a...8.Do&N
                // 0030   6d e2 29 9a 0a 85 34 00 00 00 00 00 00 00 00 00   m.)...4.........
                // 0040   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0050   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0060   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0070   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0080   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0090   00 00 00 00 00 00 20 99 6e c2 9c 8e 6d d9 85 b5   ...... .n...m...
                // 00a0   7f 03 38 61 9f 9d c2 38 d5 44 6f 26 4e 6d e2 29   ..8a...8.Do&Nm.)
                // 00b0   9a 0a 85 34 00 00 00 00 00 00 00 00 00 00 00 00   ...4............
                // 00c0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 00d0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 00e0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 00f0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0100   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0110   00 00 00 00 00 00 1d dd bf f6 fc 11 b4 1b df 86   ................
                // 0120   17 c5 24 42 ef 3c a5 9f e7 1a 7d e5 2f fa 7f 02   ..$B.<....}./...
                // 0130   27 0b df 64 ad cc 66 a4 01 01 20 04 21 58 20 c0   '..d..f... .!X .
                // 0140   58 2b 9d be 36 5d 69 64 04 96 57 b5 39 a6 fc 73   X+..6]id..W.9..s
                // 0150   6f 01 c4 e0 b4 51 25 27 8c 93 6e ef c9 80 30 6c   o....Q%'..n...0l
                // 0160   73 75 62 6a 65 63 74 20 6e 61 6d 65 60 00 00 00   subject name`...
                // 0170   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0180   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0190   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 01a0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 01b0   00 00 00 00 00 00 36 a1 04 41 17 00 00 00 00 00   ......6..A......
                // 01c0   00 00 00 00 00 00 04 52 0a d3 71 b5 a4 66 e7 f0   .......R..q..f..
                // 01d0   87 18 0e 8d 55 56 d8 4e 35 00 00 00 00 00 00 00   ....UV.N5.......
                // 01e0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 0a 83   ................
                // 01f0   68 45 6e 63 72 79 70 74 30 44 a1 04 41 17 58 58   hEncrypt0D..A.XX
                // 0200   58 20 dd bf f6 fc 11 b4 1b df 86 17 c5 24 42 ef   X ...........$B.
                // 0210   3c a5 9f e7 1a 7d e5 2f fa 7f 02 27 0b df 64 ad   <....}./...'..d.
                // 0220   cc 66 a4 01 01 20 04 21 58 20 c0 58 2b 9d be 36   .f... .!X .X+..6
                // 0230   5d 69 64 04 96 57 b5 39 a6 fc 73 6f 01 c4 e0 b4   ]id..W.9..so....
                // 0240   51 25 27 8c 93 6e ef c9 80 30 6c 73 75 62 6a 65   Q%'..n...0lsubje
                // 0250   63 74 20 6e 61 6d 65 60 4a f0 87 18 0e 8d 55 56   ct name`J.....UV
                // 0260   d8 4e 35 00 00 00 00 00 00 00 00 00 00 00 00 00   .N5.............
                // 0270   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0280   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0290   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 02a0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 02b0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 02c0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 02d0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 02e0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 02f0   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0300   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0310   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0320   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0330   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00   ................
                // 0340   00 00 00 00 00 00 00 00 00 00 00 00 74            ............t

                // warn!("eframe : {:X?}", frame);
            };

            let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
            warn!("{:?} : EAP Respone  ", timestamp);
            let _x = li.iface.lock().eap(timestamp, eapol_repr.buffer_len(), f);
        }
        sleep_until(timer_now() + Duration::from_millis(latency)).await;

        // #[cfg(target_arch = "riscv64")]
        // kernel_hal_bare::interrupt::wait_for_interrupt();
        // yield_now().await;

        // }
    }
}

async fn ping() {
    macro_rules! send_icmp_ping {
        ( $repr_type:ident, $packet_type:ident, $ident:expr, $seq_no:expr,
          $echo_payload:expr, $socket:expr, $remote_addr:expr ) => {{
            let icmp_repr = $repr_type::EchoRequest {
                ident: $ident,
                seq_no: $seq_no,
                data: &$echo_payload,
            };

            let icmp_payload = $socket.send(icmp_repr.buffer_len(), $remote_addr).unwrap();
            let icmp_packet = $packet_type::new_unchecked(icmp_payload);
            (icmp_repr, icmp_packet)
        }};
    }
    macro_rules! get_icmp_pong {
        ( $repr_type:ident, $repr:expr, $payload:expr, $waiting_queue:expr, $remote_addr:expr,
          $timestamp:expr, $received:expr ) => {{
            if let $repr_type::EchoReply { seq_no, data, .. } = $repr {
                if let Some(_) = $waiting_queue.get(&seq_no) {
                    let packet_timestamp_ms = NetworkEndian::read_i64(data);
                    warn!(
                        "{} bytes from {}: icmp_seq={}, time={}ms",
                        data.len(),
                        $remote_addr,
                        seq_no,
                        $timestamp.total_millis() - packet_timestamp_ms
                    );
                    $waiting_queue.remove(&seq_no);
                    $received += 1;
                }
            }
        }};
    }
    use alloc::collections::BTreeMap;
    use alloc::vec;
    use byteorder::{ByteOrder, NetworkEndian};
    use core::str::FromStr;
    use kernel_hal::drivers::get_net_driver;
    use kernel_hal::timer_now;
    use kernel_hal_bare::drivers::net::rtl8x::RTL8xInterface;
    use smoltcp::phy::Device;
    use smoltcp::socket::{
        IcmpEndpoint, IcmpPacketMetadata, IcmpSocket, IcmpSocketBuffer, SocketSet,
    };
    use smoltcp::time::Duration;
    use smoltcp::time::Instant;
    use smoltcp::wire::{Icmpv4Packet, Icmpv4Repr, IpAddress};
    let rtl8x = get_net_driver()[0].clone();

    let icmp_rx_buffer = IcmpSocketBuffer::new(vec![IcmpPacketMetadata::EMPTY], vec![0; 256]);
    let icmp_tx_buffer = IcmpSocketBuffer::new(vec![IcmpPacketMetadata::EMPTY], vec![0; 256]);
    let icmp_socket = IcmpSocket::new(icmp_rx_buffer, icmp_tx_buffer);

    let mut sockets = SocketSet::new(vec![]);
    let icmp_handle = sockets.add(icmp_socket);

    let mut send_at = Instant::from_millis(0);
    let mut seq_no = 0;
    let mut received = 0;
    let mut echo_payload = [0xffu8; 40];
    let mut waiting_queue = BTreeMap::new();
    let ident = 0x22b;

    let count = 4;
    // ping 的 目的 地址 、um.. 暂时手动修改吧
    // baidu
    // let ip_addr = "220.181.38.251";
    // 114 dns
    let ip_addr = "114.114.114.114";
    //let ip_addr = "192.168.0.62";
    // let ip_addr = "172.24.103.1";
    let remote_addr = IpAddress::from_str(ip_addr).expect("invalid address format");
    warn!("ping ip addr {:?}", remote_addr);
    let interval = Duration::from_secs(1);
    let timeout = Duration::from_secs(10);

    if let Ok(_li) = rtl8x.downcast_arc::<RTL8xInterface>() {
        let device = _li.iface.lock().device().clone();
        let device_caps = device.capabilities();
        let mut timeout_return: bool = false;
        loop {
            let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
            match _li.iface.lock().poll(&mut sockets, timestamp) {
                Ok(_) => {
                    // warn!("poll ok {}", b);
                }
                Err(e) => {
                    debug!("poll error: {}", e);
                }
            }

            {
                let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
                let mut socket = sockets.get::<IcmpSocket>(icmp_handle);
                if !socket.is_open() {
                    // warn!("no open");
                    // warn!("bind ident {} to icmp socket", ident);
                    socket.bind(IcmpEndpoint::Ident(ident)).unwrap();
                    send_at = timestamp;
                }

                if socket.can_send() && seq_no < count as u16 && send_at <= timestamp {
                    NetworkEndian::write_i64(&mut echo_payload, timestamp.total_millis());

                    match remote_addr {
                        IpAddress::Ipv4(addr) => {
                            // warn!("ping send addr : {}", addr);
                            let (icmp_repr, mut icmp_packet) = send_icmp_ping!(
                                Icmpv4Repr,
                                Icmpv4Packet,
                                ident,
                                seq_no,
                                echo_payload,
                                socket,
                                remote_addr
                            );
                            icmp_repr.emit(&mut icmp_packet, &device_caps.checksum);
                        }
                        _ => unimplemented!(),
                    }
                    waiting_queue.insert(seq_no, timestamp);
                    seq_no += 1;
                    send_at += interval;
                }

                if socket.can_recv() {
                    let (payload, _) = socket.recv().unwrap();

                    match remote_addr {
                        IpAddress::Ipv4(addr) => {
                            // warn!("ping recv addr : {}", addr);
                            let icmp_packet = Icmpv4Packet::new_checked(&payload).unwrap();
                            let icmp_repr =
                                Icmpv4Repr::parse(&icmp_packet, &device_caps.checksum).unwrap();
                            get_icmp_pong!(
                                Icmpv4Repr,
                                icmp_repr,
                                payload,
                                waiting_queue,
                                remote_addr,
                                timestamp,
                                received
                            );
                        }
                        _ => unimplemented!(),
                    }
                }

                // #[cfg(target_arch = "riscv64")]
                // kernel_hal_bare::interrupt::wait_for_interrupt();
                // yield_now().await;

                waiting_queue.retain(|seq, from| {
                    if timestamp - *from < timeout {
                        true
                    } else {
                        warn!("From {} icmp_seq={} timeout", remote_addr, seq);
                        timeout_return = true;
                        warn!("timeout_return {}", timeout_return);
                        false
                    }
                });

                if seq_no == count as u16 && waiting_queue.is_empty() {
                    break;
                }
            }

            if timeout_return {
                return;
            }

            #[cfg(target_arch = "riscv64")]
            kernel_hal_bare::interrupt::wait_for_interrupt();
            yield_now().await;
        }
    }
}
