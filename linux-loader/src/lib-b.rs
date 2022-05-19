//! Linux LibOS
//! - run process and manage trap/interrupt/syscall
#![no_std]
#![feature(asm)]
#![deny(warnings, unused_must_use, missing_docs)]

extern crate alloc;
#[macro_use]
extern crate log;

use {
    alloc::{boxed::Box, string::String, sync::Arc, vec::Vec},
    core::{future::Future, pin::Pin},
    kernel_hal::{GeneralRegs, MMUFlags},
    linux_object::{
        fs::{vfs::FileSystem, INodeExt},
        loader::LinuxElfLoader,
        process::ProcessExt,
        thread::ThreadExt,
    },
    linux_syscall::Syscall,
    zircon_object::task::*,
};

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
    let inode = rootfs.root_inode().lookup(&args[0]).unwrap();
    let data = inode.read_as_vec().unwrap();
    let path = args[0].clone();
    let (entry, sp) = loader.load(&proc.vmar(), &data, args, envs, path).unwrap();

    // run ping

    thread
        .start(entry, sp, 0, 0, thread_fn)
        .expect("failed to start main thread");

    // or run ping

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

        // super_mode_net_udp_server_test();
        // super_mode_net_tcp_server_test();
        // super_mode_net_tcp_client_test();
        // super_mode_frame_test();
        super_mode_eap_test().await;

        // run
        trace!("go to user: {:#x?}", cx);
        kernel_hal::context_run(&mut cx);
        trace!("back from user: {:#x?}", cx);
        // handle trap/interrupt/syscall
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
        thread.end_running(cx);
    }
}

fn thread_fn(thread: CurrentThread) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
    Box::pin(new_thread(thread))
}

/// syscall handler entry
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

#[allow(dead_code)]
fn super_mode_net_udp_server_test() {
    use core::str::from_utf8;
    use kernel_hal::devices::get_net_driver;
    use kernel_hal::timer_now;
    use kernel_hal_bare::devices::net::e1000::E1000Interface;
    //use smoltcp::socket::SocketSet;
    use smoltcp::socket::UdpPacketMetadata;
    use smoltcp::socket::UdpSocket;
    use smoltcp::socket::UdpSocketBuffer;
    //use smoltcp::time::Instant;

    // udp s
    let udp_rx_buffer = UdpSocketBuffer::new(vec![UdpPacketMetadata::EMPTY], vec![0; 64]);
    let udp_tx_buffer = UdpSocketBuffer::new(vec![UdpPacketMetadata::EMPTY], vec![0; 128]);
    // udp socket
    let udp_socket = UdpSocket::new(udp_rx_buffer, udp_tx_buffer);

    //use alloc::vec;
    // use alloc::vec::Vec;
    let mut sockets = SocketSet::new(vec![]);
    let udp_handle = sockets.add(udp_socket);

    let e1000 = get_net_driver()[0].clone();
    // let local = NET_DRIVERS.read()[0].clone();

    if let Ok(_li) = e1000.downcast_arc::<E1000Interface>() {
        // use std::os::unix::io::AsRawFd;
        // let fd = li.iface.lock().device().as_raw_fd();
        // let mut sockets = SOCKETS.lock();
        loop {
            // let _timestamp = Instant::now();
            let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
            match _li.iface.lock().poll(&mut sockets, timestamp) {
                Ok(_a) => {
                    // warn!("poll ok: {}", a);
                }
                Err(e) => {
                    warn!("poll error: {}", e);
                }
            }
            {
                // udp bind
                let mut socket = sockets.get::<UdpSocket>(udp_handle);
                if !socket.is_open() {
                    warn!("bind 6969");
                    socket.bind(6969).unwrap()
                }
                // udp recv
                let client = match socket.recv() {
                    Ok((data, endpoint)) => {
                        warn!(" udp recv : {} form {}", from_utf8(data).unwrap(), endpoint);
                        Some(endpoint)
                    }
                    Err(_) => None,
                };
                // udp send
                if let Some(endpoint) = client {
                    let data = b"cargo test OK";
                    warn!(
                        "udp:6969 send data: {:?}",
                        from_utf8(data.as_ref()).unwrap()
                    );
                    socket.send_slice(data, endpoint).unwrap();
                }
            }
        }
    }
    todo!();
}

// #[allow(dead_code)]
// fn super_mode_net_tcp_server_test() {
//     use smoltcp::socket::TcpSocket;
//     use smoltcp::socket::TcpSocketBuffer;
//     use smoltcp::time::Instant;
//     use core::str::from_utf8;
//     use kernel_hal::devices::get_net_driver;
//     use kernel_hal::timer_now;
//     use kernel_hal_bare::devices::net::e1000::E1000Interface;
//     use smoltcp::socket::SocketSet;

//     let tcp1_rx_buffer = TcpSocketBuffer::new(vec![0; 64]);
//     let tcp1_tx_buffer = TcpSocketBuffer::new(vec![0; 128]);
//     let tcp1_socket = TcpSocket::new(tcp1_rx_buffer, tcp1_tx_buffer);

//     use alloc::vec;
//     // use alloc::vec::Vec;
//     let mut sockets = SocketSet::new(vec![]);
//     let tcp1_handle = sockets.add(tcp1_socket);

//     let e1000 = get_net_driver()[0].clone();
//         // let local = NET_DRIVERS.read()[0].clone();

//     if let Ok(_li) = e1000.downcast_arc::<E1000Interface>() {
//             // use std::os::unix::io::AsRawFd;
//             // let fd = li.iface.lock().device().as_raw_fd();
//             // let mut sockets = SOCKETS.lock();
//         loop {
//             // let _timestamp = Instant::now();
//             let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
//             match _li.iface.lock().poll(&mut sockets, timestamp) {
//                 Ok(_a) => {
//                     // warn!("poll ok: {}", a);
//                 }
//                 Err(e) => {
//                     warn!("poll error: {}", e);
//                 }
//             }
//             {
//                 let mut socket = sockets.get::<TcpSocket>(tcp1_handle);
//                 if !socket.is_open() {
//                     socket.listen(6969).unwrap();
//                 }

//                 if socket.can_send() {
//                     let data = b"cargo tcp test OK";
//                     warn!(
//                         "server tcp:6969 send data: {:?}",
//                         from_utf8(data.as_ref()).unwrap()
//                     );
//                     socket.send_slice(&data[..]).unwrap();
//                     warn!("tcp:6969 close");
//                     socket.close();
//                 }
//             }
//         }
//     }
//     todo!();

// }

#[allow(dead_code)]
fn super_mode_net_tcp_client_test() {
    use alloc::borrow::ToOwned;
    use core::str::from_utf8;
    use kernel_hal::devices::get_net_driver;
    use kernel_hal::timer_now;
    use kernel_hal_bare::devices::net::e1000::E1000Interface;
    //use smoltcp::socket::SocketSet;
    use smoltcp::socket::TcpSocket;
    use smoltcp::socket::TcpSocketBuffer;
    //use smoltcp::time::Instant;
    //use smoltcp::wire::IpAddress;

    let tcp_rx_buffer = TcpSocketBuffer::new(vec![0; 64]);
    let tcp_tx_buffer = TcpSocketBuffer::new(vec![0; 128]);
    let tcp_socket = TcpSocket::new(tcp_rx_buffer, tcp_tx_buffer);

    //use alloc::vec;
    // use alloc::vec::Vec;
    let mut sockets = SocketSet::new(vec![]);
    let tcp_handle = sockets.add(tcp_socket);

    let e1000 = get_net_driver()[0].clone();
    // let local = NET_DRIVERS.read()[0].clone();

    if let Ok(_li) = e1000.downcast_arc::<E1000Interface>() {
        let addr = IpAddress::v4(172, 25, 220, 230);
        let port = 6969u16;
        {
            let mut socket = sockets.get::<TcpSocket>(tcp_handle);
            socket.connect((addr, port), 49500).unwrap();
        }

        let mut tcp_active = false;
        loop {
            // let _timestamp = Instant::now();
            let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
            match _li.iface.lock().poll(&mut sockets, timestamp) {
                Ok(_a) => {
                    // warn!("poll ok: {}", a);
                }
                Err(e) => {
                    warn!("poll error: {}", e);
                }
            }
            {
                let mut socket = sockets.get::<TcpSocket>(tcp_handle);
                if socket.is_active() && !tcp_active {
                    warn!("connected");
                } else if !socket.is_active() && tcp_active {
                    warn!("disconnected");
                    // break
                }

                tcp_active = socket.is_active();

                if socket.may_send() {
                    warn!("send");
                    let mut data: Vec<u8> = Vec::new();
                    data.push(100);
                    socket.send_slice(&data[..]).unwrap();
                }

                if socket.may_recv() {
                    let data = socket
                        .recv(|data| {
                            let mut data = data.to_owned();
                            if !data.is_empty() {
                                warn!(
                                    "recv data: {:?}",
                                    from_utf8(data.as_ref()).unwrap_or("(invalid utf8)")
                                );
                                data = data.split(|&b| b == b'\n').collect::<Vec<_>>().concat();
                                data.reverse();
                                data.extend(b"\n");
                            }
                            (data.len(), data)
                        })
                        .unwrap_or(vec![99]);
                    if socket.can_send() && !data.is_empty() {
                        warn!(
                            "send data: {:?}",
                            from_utf8(data.as_ref()).unwrap_or("(invalid utf8)")
                        );
                        socket.send_slice(&data[..]).unwrap();
                    }
                } else if socket.may_send() {
                    warn!("close");
                    socket.close();
                }
            }
        }
    }
    todo!();
}

#[allow(dead_code)]
#[allow(unused_imports)]
#[allow(unused_variables)]
fn super_mode_frame_test() {
    use core::str::from_utf8;
    use kernel_hal::devices::get_net_driver;
    use kernel_hal::timer_now;
    use kernel_hal_bare::devices::net::e1000::E1000Interface;
    use smoltcp::socket::SocketSet;
    use smoltcp::socket::UdpPacketMetadata;
    use smoltcp::socket::UdpSocket;
    use smoltcp::socket::UdpSocketBuffer;
    use smoltcp::time::Instant;

    // // udp s
    // let udp_rx_buffer = UdpSocketBuffer::new(vec![UdpPacketMetadata::EMPTY], vec![0; 64]);
    // let udp_tx_buffer = UdpSocketBuffer::new(vec![UdpPacketMetadata::EMPTY], vec![0; 128]);
    // // udp socket
    // let udp_socket = UdpSocket::new(udp_rx_buffer, udp_tx_buffer);

    // use alloc::vec;
    // // use alloc::vec::Vec;
    // let mut sockets = SocketSet::new(vec![]);
    // let udp_handle = sockets.add(udp_socket);

    let e1000 = get_net_driver()[0].clone();
    // let local = NET_DRIVERS.read()[0].clone();

    if let Ok(_li) = e1000.downcast_arc::<E1000Interface>() {
        // use std::os::unix::io::AsRawFd;
        // let fd = li.iface.lock().device().as_raw_fd();
        // let mut sockets = SOCKETS.lock();
        loop {
            let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
            let _x = _li.iface.lock().biubiu(timestamp);
        }
        // loop {
        //     // let _timestamp = Instant::now();
        //     let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
        //     let _x = _li.iface.lock().biubiu(timestamp);
        //     match _li.iface.lock().poll(&mut sockets, timestamp) {
        //         Ok(_a) => {
        //             // warn!("poll ok: {}", a);
        //         }
        //         Err(e) => {
        //             warn!("poll error: {}", e);
        //         }
        //     }
        //     {
        //         // udp bind
        //         let mut socket = sockets.get::<UdpSocket>(udp_handle);
        //         if !socket.is_open() {
        //             warn!("bind 6969");
        //             socket.bind(6969).unwrap()
        //         }
        //          // udp recv
        //         let client = match socket.recv() {
        //             Ok((data, endpoint)) => {
        //                 warn!(" udp recv : {} form {}", from_utf8(data).unwrap(), endpoint);
        //                 Some(endpoint)
        //             }
        //             Err(_) => None,
        //         };
        //         // udp send
        //         if let Some(endpoint) = client {
        //             let data = b"cargo test OK";
        //             warn!(
        //                 "udp:6969 send data: {:?}",
        //                 from_utf8(data.as_ref()).unwrap()
        //             );
        //             socket.send_slice(data, endpoint).unwrap();
        //         }
        //     }
        // }
        // loop{}
    }
    todo!();
}

#[allow(dead_code)]
#[allow(unused_imports)]
#[allow(unused_variables)]
async fn super_mode_eap_test() {
    use core::str::from_utf8;
    use kernel_hal::devices::get_net_driver;
    use kernel_hal::timer_now;
    use kernel_hal_bare::devices::net::e1000::E1000Interface;
    use smoltcp::socket::SocketSet;
    use smoltcp::socket::UdpPacketMetadata;
    use smoltcp::socket::UdpSocket;
    use smoltcp::socket::UdpSocketBuffer;
    use smoltcp::time::Instant;

    use smoltcp::wire::*;

    use core::time::Duration;
    use kernel_hal::sleep_until;
    use kernel_hal::timer_tick;

    let latency : u64 = 300;

    let mac_addr = EthernetAddress([0x01, 0x80, 0xc2, 0x00, 0x00, 0x03]);

    let e1000 = get_net_driver()[0].clone();

    if let Ok(li) = e1000.downcast_arc::<E1000Interface>() {
        loop {
        let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
        let _x = li.iface.lock().eapol(timestamp); // eapol start

        info!("<-- EAPOL Start from smoltcp");

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
                identifier: 4,
                len: 7,
            };

            let eap_data_repr = EAPDataRepr::EthernetEAPData {
                eapdata_type: EAPDataType::Identifier,
            };
            let f = |mut frame: EthernetFrame<&mut [u8]>| {
                //frame.set_dst_addr(EthernetAddress::BROADCAST);
                frame.set_dst_addr(mac_addr);
                frame.set_ethertype(EthernetProtocol::EAPoL);

                let mut eapol_packet = EAPoLPacket::new_unchecked(frame.payload_mut());
                eapol_repr.emit(&mut eapol_packet);
                let mut eap_packet = EAPPacket::new_unchecked(eapol_packet.packet_mut());
                eap_repr.emit(&mut eap_packet);
                let mut eap_data = EAPDataPacket::new_unchecked(eap_packet.data_mut());
                eap_data_repr.emit(&mut eap_data);

                /*
                let typedata: [u8; 16] = [
                    0x43, 0x68, 0x65, 0x6e, 0x20, 0x58, 0x69, 0x6E, 0x67, 0x20, 0x7A, 0x43, 0x6F,
                    0x72, 0x65, 0x31,
                ];
                */
                let typedata: [u8; 2] = [0x63, 0x6a]; //Identity: cj

                eap_data.typedata_mut().copy_from_slice(&typedata[..]);

                info!("EAP Response, eframe : {:x?}", frame);
            };

            let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
            let _x = li.iface.lock().eap(timestamp, eapol_repr.buffer_len(), f);
        }

        // 等待
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
                identifier: 5,
                len: 181,
            };

            let eap_data_repr = EAPDataRepr::EthernetEAPData {
                eapdata_type: EAPDataType::UnknownType,
            };
            let f = |mut frame: EthernetFrame<&mut [u8]>| {
                //frame.set_dst_addr(EthernetAddress::BROADCAST);
                frame.set_dst_addr(mac_addr);
                frame.set_ethertype(EthernetProtocol::EAPoL);

                let mut eapol_packet = EAPoLPacket::new_unchecked(frame.payload_mut());
                eapol_repr.emit(&mut eapol_packet);
                let mut eap_packet = EAPPacket::new_unchecked(eapol_packet.packet_mut());
                eap_repr.emit(&mut eap_packet);
                let mut eap_data = EAPDataPacket::new_unchecked(eap_packet.data_mut());
                eap_data_repr.emit(&mut eap_data);

                // 0d00000001000000820000c0582b9dbe365d6964049657b539a6fc736f01c4e0b45125278c936eefc98030070e000d005820c0582b9dbe365d6964049657b539a6fc736f01c4e0b45125278c936eefc9803030000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000250000

                let typedata: [u8; 176] = [0x0d, 0,0,0,0x01, 0,0,0,0x82, 0,0,0xc0, 0x58, 0x2b, 0x9d, 0xbe, 0x36, 0x5d, 0x69, 0x64, 0x04, 0x96, 0x57, 0xb5, 0x39, 0xa6, 0xfc, 0x73, 0x6f, 0x01, 0xc4, 0xe0, 0xb4, 0x51, 0x25, 0x27, 0x8c, 0x93, 0x6e, 0xef, 0xc9, 0x80, 0x30, 0x07, 0x0e, 0, 0x0d, 0, 0x58, 0x20, 0xc0, 0x58, 0x2b, 0x9d, 0xbe, 0x36, 0x5d, 0x69, 0x64, 0x04, 0x96, 0x57, 0xb5, 0x39, 0xa6, 0xfc, 0x73, 0x6f, 0x01, 0xc4, 0xe0, 0xb4, 0x51, 0x25, 0x27, 0x8c, 0x93, 0x6e, 0xef, 0xc9, 0x80, 0x30, 0x30, 0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0x25,0,0];
                // let typedata: [u8; 15] = [0x43 ,0x68 ,0x20 ,0x20 ,0x20 ,0x58 ,0x69 ,0x6E ,0x67 ,0x20 ,0x7A ,0x43 ,0x6F ,0x72 ,0x65];

                eap_data.typedata_mut().copy_from_slice(&typedata[..]);
                info!("EAP Response, eframe : {:x?}", frame);
            };

            let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
            let _x = li.iface.lock().eap(timestamp, eapol_repr.buffer_len(), f);
        }

        // 等待
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
                identifier: 6,
                len: 827,
            };

            let eap_data_repr = EAPDataRepr::EthernetEAPData {
                eapdata_type: EAPDataType::UnknownType,
            };
            let f = |mut frame: EthernetFrame<&mut [u8]>| {
                //frame.set_dst_addr(EthernetAddress::BROADCAST);
                frame.set_dst_addr(mac_addr);
                frame.set_ethertype(EthernetProtocol::EAPoL);

                let mut eapol_packet = EAPoLPacket::new_unchecked(frame.payload_mut());
                eapol_repr.emit(&mut eapol_packet);
                let mut eap_packet = EAPPacket::new_unchecked(eapol_packet.packet_mut());
                eap_repr.emit(&mut eap_packet);
                let mut eap_data = EAPDataPacket::new_unchecked(eap_packet.data_mut());
                eap_data_repr.emit(&mut eap_data);

                // 29581ef56be7dbc35aa776473712e35b3c0ac07f617a3c753110afa99790f8e4a80000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000021f56be7dbc35aa776473712e35b3c0ac07f617a3c753110afa99790f8e4a8000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001eeb6a0f36cab86e18f198d5f460a022bce63634f530e926ea6ab629b1da08cad0a401012004215820c0582b9dbe365d6964049657b539a6fc736f01c4e0b45125278c936eefc980306c7375626a656374206e616d65600000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000036a104411700000000000000000000000431e29aa20c17cae1ce9af087180e8d5556d84e00000000000000000000000000000000000000000b8368456e63727970743044a104411758585820eb6a0f36cab86e18f198d5f460a022bce63634f530e926ea6ab629b1da08cad0a401012004215820c0582b9dbe365d6964049657b539a6fc736f01c4e0b45125278c936eefc980306c7375626a656374206e616d65604bce9af087180e8d5556d84e0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000075
                let typedata: [u8; 822] =
                    [0x29, 0x58, 0x1e, 0xf5, 0x6b, 0xe7, 0xdb, 0xc3, 0x5a, 0xa7, 0x76, 0x47, 0x37, 0x12, 0xe3, 0x5b, 0x3c, 0x0a, 0xc0, 0x7f, 0x61, 0x7a, 0x3c, 0x75, 0x31, 0x10, 0xaf, 0xa9, 0x97, 0x90, 0xf8, 0xe4, 0xa8, 0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0, 0x21, 0xf5, 0x6b, 0xe7, 0xdb, 0xc3, 0x5a, 0xa7, 0x76, 0x47, 0x37, 0x12, 0xe3, 0x5b, 0x3c, 0x0a, 0xc0, 0x7f, 0x61, 0x7a, 0x3c, 0x75, 0x31, 0x10, 0xaf, 0xa9, 0x97, 0x90, 0xf8, 0xe4, 0xa8, 0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0, 0x1e, 0xeb, 0x6a, 0x0f, 0x36, 0xca, 0xb8, 0x6e, 0x18, 0xf1, 0x98, 0xd5, 0xf4, 0x60, 0xa0, 0x22, 0xbc, 0xe6, 0x36, 0x34, 0xf5, 0x30, 0xe9, 0x26, 0xea, 0x6a, 0xb6, 0x29, 0xb1, 0xda, 0x08, 0xca, 0xd0, 0xa4, 0x01, 0x01, 0x20, 0x04, 0x21, 0x58, 0x20, 0xc0, 0x58, 0x2b, 0x9d, 0xbe, 0x36, 0x5d, 0x69, 0x64, 0x04, 0x96, 0x57, 0xb5, 0x39, 0xa6, 0xfc, 0x73, 0x6f, 0x01, 0xc4, 0xe0, 0xb4, 0x51, 0x25, 0x27, 0x8c, 0x93, 0x6e, 0xef, 0xc9, 0x80, 0x30, 0x6c, 0x73, 0x75, 0x62, 0x6a, 0x65, 0x63, 0x74, 0x20, 0x6e, 0x61, 0x6d, 0x65, 0x60, 0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0, 0x36, 0xa1, 0x04, 0x41, 0x17, 0,0,0,0,0,0,0,0,0,0,0, 0x04, 0x31, 0xe2, 0x9a, 0xa2, 0x0c, 0x17, 0xca, 0xe1, 0xce, 0x9a, 0xf0, 0x87, 0x18, 0x0e, 0x8d, 0x55, 0x56, 0xd8, 0x4e, 0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0, 0x0b, 0x83, 0x68, 0x45, 0x6e, 0x63, 0x72, 0x79, 0x70, 0x74, 0x30, 0x44, 0xa1, 0x04, 0x41, 0x17, 0x58, 0x58, 0x58, 0x20, 0xeb, 0x6a, 0x0f, 0x36, 0xca, 0xb8, 0x6e, 0x18, 0xf1, 0x98, 0xd5, 0xf4, 0x60, 0xa0, 0x22, 0xbc, 0xe6, 0x36, 0x34, 0xf5, 0x30, 0xe9, 0x26, 0xea, 0x6a, 0xb6, 0x29, 0xb1, 0xda, 0x08, 0xca, 0xd0, 0xa4, 0x01, 0x01, 0x20, 0x04, 0x21, 0x58, 0x20, 0xc0, 0x58, 0x2b, 0x9d, 0xbe, 0x36, 0x5d, 0x69, 0x64, 0x04, 0x96, 0x57, 0xb5, 0x39, 0xa6, 0xfc, 0x73, 0x6f, 0x01, 0xc4, 0xe0, 0xb4, 0x51, 0x25, 0x27, 0x8c, 0x93, 0x6e, 0xef, 0xc9, 0x80, 0x30, 0x6c, 0x73, 0x75, 0x62, 0x6a, 0x65, 0x63, 0x74, 0x20, 0x6e, 0x61, 0x6d, 0x65, 0x60, 0x4b, 0xce, 0x9a, 0xf0, 0x87, 0x18, 0x0e, 0x8d, 0x55, 0x56, 0xd8, 0x4e, 0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0x75];

                eap_data.typedata_mut().copy_from_slice(&typedata[..]);

                info!("EAP Response, eframe : {:x?}", frame);
            };

            let timestamp = Instant::from_millis(timer_now().as_millis() as i64);
            let _x = li.iface.lock().eap(timestamp, eapol_repr.buffer_len(), f);
        }

        // 等待30s
        sleep_until(timer_now() + Duration::from_millis(30000)).await;

        }//loop
    }
    loop {}

    // todo!();
}


/////////

use alloc::vec;
use alloc::collections::BTreeMap;
use hashbrown::HashMap;
use byteorder::{ByteOrder, NetworkEndian};


use smoltcp::iface::{InterfaceBuilder, NeighborCache, Routes};
//use smoltcp::phy::wait as phy_wait;
//use smoltcp::phy::Device;
use smoltcp::socket::{IcmpEndpoint, IcmpPacketMetadata, IcmpSocket, IcmpSocketBuffer, SocketSet};
use smoltcp::wire::{
    EthernetAddress, Icmpv4Packet, Icmpv4Repr, Icmpv6Packet, Icmpv6Repr, IpAddress, IpCidr,
    Ipv4Address, Ipv6Address,
};
use smoltcp::{
    phy::Medium,
    time::{Duration, Instant},
};

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
                info!(
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

fn ping() {

    //let address = IpAddress::from_str("192.168.1.100"); //remote addr
    let address = IpAddress::v4(192, 168, 1, 100);
    let count = 4;

    let interval = Duration::from_secs(1);
    let timeout = Duration::from_secs(5);

    let neighbor_cache = NeighborCache::new(BTreeMap::new());

    let remote_addr = address;

    let icmp_rx_buffer = IcmpSocketBuffer::new(vec![IcmpPacketMetadata::EMPTY], vec![0; 256]);
    let icmp_tx_buffer = IcmpSocketBuffer::new(vec![IcmpPacketMetadata::EMPTY], vec![0; 256]);
    let icmp_socket = IcmpSocket::new(icmp_rx_buffer, icmp_tx_buffer);

    let ethernet_addr = EthernetAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x02]);
    let src_ipv6 = IpAddress::v6(0xfdaa, 0, 0, 0, 0, 0, 0, 1);
    let ip_addrs = [
        IpCidr::new(IpAddress::v4(192, 168, 1, 10), 24),
        IpCidr::new(src_ipv6, 64),
        IpCidr::new(IpAddress::v6(0xfe80, 0, 0, 0, 0, 0, 0, 1), 64),
    ];
    let default_v4_gw = Ipv4Address::new(192, 168, 1, 100);
    let default_v6_gw = Ipv6Address::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x100);
    let mut routes_storage = [None; 2];
    let mut routes = Routes::new(&mut routes_storage[..]);
    routes.add_default_ipv4_route(default_v4_gw).unwrap();
    routes.add_default_ipv6_route(default_v6_gw).unwrap();

    let medium = device.capabilities().medium;
    let mut builder = InterfaceBuilder::new(device)
        .ip_addrs(ip_addrs)
        .routes(routes);
    if medium == Medium::Ethernet {
        builder = builder
            .ethernet_addr(ethernet_addr)
            .neighbor_cache(neighbor_cache);
    }
    let mut iface = builder.finalize();

    let mut sockets = SocketSet::new(vec![]);
    let icmp_handle = sockets.add(icmp_socket);

    let mut send_at = Instant::from_millis(0);
    let mut seq_no = 0;
    let mut received = 0;
    let mut echo_payload = [0xffu8; 40];
    let mut waiting_queue = HashMap::new();
    let ident = 0x22b;

    loop {
        //let timestamp = Instant::now(); // std? xly 
        let timestamp = Instant::from_millis(kernel_hal::timer_now().as_millis() as i64);

        match iface.poll(&mut sockets, timestamp) {
            Ok(_) => {}
            Err(e) => {
                debug!("poll error: {}", e);
            }
        }

        {
            //let timestamp = Instant::now();
            let timestamp = Instant::from_millis(kernel_hal::timer_now().as_millis() as i64);
            let mut socket = sockets.get::<IcmpSocket>(icmp_handle);
            if !socket.is_open() {
                socket.bind(IcmpEndpoint::Ident(ident)).unwrap();
                send_at = timestamp;
            }

            if socket.can_send() && seq_no < count as u16 && send_at <= timestamp {
                NetworkEndian::write_i64(&mut echo_payload, timestamp.total_millis());

                match remote_addr {
                    IpAddress::Ipv4(_) => {
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
                    IpAddress::Ipv6(_) => {
                        let (icmp_repr, mut icmp_packet) = send_icmp_ping!(
                            Icmpv6Repr,
                            Icmpv6Packet,
                            ident,
                            seq_no,
                            echo_payload,
                            socket,
                            remote_addr
                        );
                        icmp_repr.emit(
                            &src_ipv6,
                            &remote_addr,
                            &mut icmp_packet,
                            &device_caps.checksum,
                        );
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
                    IpAddress::Ipv4(_) => {
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
                    IpAddress::Ipv6(_) => {
                        let icmp_packet = Icmpv6Packet::new_checked(&payload).unwrap();
                        let icmp_repr = Icmpv6Repr::parse(
                            &remote_addr,
                            &src_ipv6,
                            &icmp_packet,
                            &device_caps.checksum,
                        )
                        .unwrap();
                        get_icmp_pong!(
                            Icmpv6Repr,
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

            waiting_queue.retain(|seq, from| {
                if timestamp - *from < timeout {
                    true
                } else {
                    info!("From {} icmp_seq={} timeout", remote_addr, seq);
                    false
                }
            });

            if seq_no == count as u16 && waiting_queue.is_empty() {
                break;
            }
        }

        //let timestamp = Instant::now();
        let timestamp = Instant::from_millis(kernel_hal::timer_now().as_millis() as i64);
        match iface.poll_at(&sockets, timestamp) {
            Some(poll_at) if timestamp < poll_at => {
                // ? xly
                //let resume_at = cmp::min(poll_at, send_at);
                //phy_wait(fd, Some(resume_at - timestamp)).expect("wait error");
            }
            Some(_) => (),
            None => {
                // ? xly
                //phy_wait(fd, Some(send_at - timestamp)).expect("wait error");
            }
        }
    }

    info!("--- {} ping statistics ---", remote_addr);
    info!(
        "{} packets transmitted, {} received, {:.0}% packet loss",
        seq_no,
        received,
        100.0 * (seq_no - received) as f64 / seq_no as f64
    );
}
