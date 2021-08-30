use smoltcp::phy::loopback::Loopback;
// struct LoopBackInterface {
//     pub iface: Mutex<Interface<'static, Loopback>
// }

#[export_name = "hal_net_loopback_init"]
pub fn init(name: String, irq: Option<usize>, header: usize, size: usize, index: usize) {
    warn!("loop back", name);

    let device = Loopback::new(Medium::Ethernet);

    let mut neighbor_cache_entries = [None; 8];
    let mut neighbor_cache = NeighborCache::new(&mut neighbor_cache_entries[..]);

    let mut ip_addrs = [IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8)];
    let mut iface = InterfaceBuilder::new(device)
        .ethernet_addr(EthernetAddress::default())
        .neighbor_cache(neighbor_cache)
        .ip_addrs(ip_addrs)
        .finalize();
    // randomly generated
    let mac: [u8; 6] = [0x54, 0x51, 0x9F, 0x71, 0xC0, index as u8];

    let e1000 = E1000::new(header, size, DriverEthernetAddress::from_bytes(&mac));

    let net_driver = E1000Driver(Arc::new(Mutex::new(e1000)));

    let ethernet_addr = EthernetAddress::from_bytes(&mac);
    let ip_addrs = [IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24)];
    let default_gateway = Ipv4Address::new(10, 0, 2, 2);
    let neighbor_cache = NeighborCache::new(BTreeMap::new());
    static mut routes_storage: [Option<(IpCidr, Route)>; 1] = [None; 1];
    let mut routes = unsafe { Routes::new(&mut routes_storage[..]) };
    routes.add_default_ipv4_route(default_gateway).unwrap();
    let iface = InterfaceBuilder::new(net_driver.clone())
        .ethernet_addr(ethernet_addr)
        .ip_addrs(ip_addrs)
        .routes(routes)
        .neighbor_cache(neighbor_cache)
        .finalize();

    warn!("e1000 interface {} up with addr 10.0.{}.2/24", name, index);
    let e1000_iface = E1000Interface {
        iface: Mutex::new(iface),
        driver: net_driver.clone(),
        name,
        irq,
    };

    // #[cfg(target_arch = "x86_64")]
    // use crate::arch::x86_64::interrupt::irq_add_handle;
    // irq_add_handle(57,e1000_iface.try_handle_interrupt());
    let driver = Arc::new(e1000_iface);
    NET_DRIVERS.write().push(driver);
}
