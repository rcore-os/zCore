use kernel_hal::drivers::NET_DRIVERS;
use kernel_hal::drivers::SOCKETS;
use kernel_hal::{NetDriver, Thread, yield_now};
use alloc::vec;
use alloc::sync::Arc;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use core::fmt::Write;
use smoltcp::socket::*;
use smoltcp::time::Instant;
use smoltcp::iface::{InterfaceBuilder, NeighborCache};
use smoltcp::wire::{IpAddress, IpCidr};

async fn server(_arg: usize) {

    //判断Vec中是否有保存初始化好的驱动
    if NET_DRIVERS.read().len() < 1 {
        loop {
            #[cfg(target_arch = "riscv64")]
            kernel_hal_bare::interrupt::wait_for_interrupt();
            //让另外一个shell线程在陷入syscall时，也可以接收到中断

            trace!("NO NET DRIVERS !");
            yield_now().await;
        }
    }

    //use kernel_hal_bare::drivers::net::virtio_net::VirtIONetDriver as DriverInterface;
    use kernel_hal_bare::drivers::net::rtl8x::RTL8xInterface as DriverInterface;

    // Ref: https://github.com/elliott10/rCore/blob/6f1953b9773d66cf7ab831c345a44e89036751c1/kernel/src/net/test.rs
    let driver = {
        //选第一个网卡驱动
        let ref_driver = Arc::clone(&NET_DRIVERS.write()[0]);
                                            //需实现Clone
        ref_driver.as_any().downcast_ref::<DriverInterface>().unwrap().clone()
    };
    let ethernet_addr = driver.get_mac();
    let ifname = driver.get_ifname();

    debug!("NET_DRIVERS read OK!\n{} MAC: {:x?}", ifname, ethernet_addr);
    debug!("IP address: {:?}", driver.get_ip_addresses());
    let mut iface = driver.iface.lock();

    /*
    //let hw_addr = EthernetAddress::from_bytes(&mac);
    let hw_addr = ethernet_addr;
    let neighbor_cache = NeighborCache::new(BTreeMap::new());
    //let ip_addrs = [IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24)];
    let ip_addrs = [IpCidr::new(IpAddress::v4(192, 168, 100, 10), 24)];

    let mut iface = InterfaceBuilder::new(driver)
        .ethernet_addr(hw_addr)
        .neighbor_cache(neighbor_cache)
        .ip_addrs(ip_addrs)
        .finalize();
    */

    // Ethernet Frame testing

    //需要std ???
    /*
    let ifname = env::args().nth(1).unwrap();
    let mut socket = RawSocket::new(ifname.as_ref()).unwrap();
    loop {
        phy_wait(socket.as_raw_fd(), None).unwrap();
        let (rx_token, _) = socket.receive().unwrap();
        rx_token
            .consume(Instant::now(), |buffer| {
                println!(
                    "{}",
                    PrettyPrinter::<EthernetFrame<&[u8]>>::new("", &buffer)
                );
                Ok(())
            })
            .unwrap();
    }
    */

    let udp_rx_buffer = UdpSocketBuffer::new(vec![UdpPacketMetadata::EMPTY], vec![0; 64]);
    let udp_tx_buffer = UdpSocketBuffer::new(vec![UdpPacketMetadata::EMPTY], vec![0; 128]);
    let udp_socket = UdpSocket::new(udp_rx_buffer, udp_tx_buffer);

    let tcp_rx_buffer = TcpSocketBuffer::new(vec![0; 1024]);
    let tcp_tx_buffer = TcpSocketBuffer::new(vec![0; 1024]);
    let tcp_socket = TcpSocket::new(tcp_rx_buffer, tcp_tx_buffer);

    let tcp2_rx_buffer = TcpSocketBuffer::new(vec![0; 1024]);
    let tcp2_tx_buffer = TcpSocketBuffer::new(vec![0; 1024]);
    let tcp2_socket = TcpSocket::new(tcp2_rx_buffer, tcp2_tx_buffer);

    let mut sockets = SOCKETS.lock();
    let udp_handle = sockets.add(udp_socket);
    let tcp_handle = sockets.add(tcp_socket);
    let tcp2_handle = sockets.add(tcp2_socket);
    drop(sockets);

    loop
    {
        {
            let mut sockets = SOCKETS.lock();

            let timestamp = Instant::from_millis(0);
            //poll一般不要被阻塞,以便可以响应下列监听的网络协议
            match iface.poll(&mut sockets, timestamp) {
                Ok(_) => {},
                Err(e) => {
                    error!("poll error: {}", e);
                }
            }

            // udp server
            {
                let mut socket = sockets.get::<UdpSocket>(udp_handle);
                if !socket.is_open() {
                    socket.bind(6969).unwrap();
                    debug!("UDP bind port 6969");
                }

                let client = match socket.recv() {
                    Ok((_, endpoint)) => Some(endpoint),
                    Err(_) => None,
                };
                if let Some(endpoint) = client {
                    info!("UDP 6969 recv");
                    let hello = b"hello from zCore\n";
                    socket.send_slice(hello, endpoint).unwrap();
                }
            }

            // simple http server
            {
                let mut socket = sockets.get::<TcpSocket>(tcp_handle);
                if !socket.is_open() {
                    socket.listen(80).unwrap();
                    debug!("TCP listen port 80");
                }

                if socket.can_send() {
                    info!("TCP 80 recv");
                    write!(socket, "HTTP/1.1 200 OK\r\nServer: zCore\r\nContent-Length: 13\r\nContent-Type: text/html\r\nConnection: Closed\r\n\r\nHello! zCore \r\n").unwrap();
                    socket.close();
                }
            }

            // simple tcp server that just eats everything
            {
                let mut socket = sockets.get::<TcpSocket>(tcp2_handle);
                if !socket.is_open() {
                    socket.listen(2222).unwrap();
                    debug!("TCP listen port 2222");
                }

                if socket.can_recv() {
                    info!("TCP 2222 recv");
                    let mut data = [0u8; 2048];
                    let _size = socket.recv_slice(&mut data).unwrap();

                    let mut linebuf: [char; 16] = [0 as char; 16];
                    for i in 0..linebuf.len() {
                        linebuf[i] = data[i] as char;
                    }
                    info!("Got: {:?}", linebuf);
                }
            }
        }

        //一般大量循环打印是正常状态
        trace!("--- loop() ---");

        #[cfg(target_arch = "riscv64")]
        kernel_hal_bare::interrupt::wait_for_interrupt();

        yield_now().await;
    }
}

pub fn net_start_thread() {
    let pin_future = Box::pin(server(0));
    let vmtoken = kernel_hal::current_page_table();
    Thread::spawn(pin_future, vmtoken);
}
