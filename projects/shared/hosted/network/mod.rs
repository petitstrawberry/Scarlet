//! Hosted Ethernet device backed by a user-mode NAT transport.

mod slirp;

use scarlet::{
    arch::Trapframe,
    device::{
        Device, DeviceType,
        manager::DeviceManager,
        network::{
            DevicePacket, EthernetDevice, LinkObservation, MacAddress, NetworkDevice,
            NetworkInterfaceConfig, NetworkStats,
        },
    },
    interrupt::{CpuId, InterruptClaim, InterruptId, InterruptManager, InterruptResult, InterruptSource},
    network::{
        NetworkManager, TX_QUEUE_FULL,
        config::apply_pending_ip_for_interface,
        ethernet::EthernetLayer,
        ethernet_interface::EthernetNetworkInterface,
        receive::{enqueue_rx_packets, ensure_rx_worker_started},
        tcp::notify_transmit_available,
    },
    object::capability::{
        ControlOps, MemoryMappingInfo, MemoryMappingOps, ReadyInterest, SelectWaitOutcome,
        Selectable,
    },
    sync::IrqSpinLock,
};
use std::{
    any::Any,
    io::{self, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

const NAME: &str = "eth0";
const MTU: usize = 1500;
const HEADER: usize = 14;
const QUEUE_CAPACITY: usize = 256;
/// Register the shared Ethernet endpoint and its BSP-assigned IRQ route.
pub fn attach(cpu: CpuId, line: InterruptId, library: PathBuf) -> Result<(), String> {
    let manager = NetworkManager::get_manager();
    let devices = DeviceManager::get_manager();
    if manager.get_interface(NAME).is_some() || devices.get_device_by_name(NAME).is_some() {
        return Err(format!("Interface {NAME} is already registered"));
    }
    let layer = manager
        .get_layer("ethernet")
        .ok_or("Ethernet was not initialized")?;
    let ethernet = layer
        .as_any()
        .downcast_ref::<EthernetLayer>()
        .ok_or("Unexpected Ethernet layer")?;

    let (tx, host_tx) = mpsc::sync_channel(QUEUE_CAPACITY);
    let (host_rx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
    let (kick, wake) = UnixStream::pair().map_err(|error| error.to_string())?;
    kick.set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    wake.set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let carrier = Arc::new(Carrier::new(cpu, line));
    let device = Arc::new(HostedEthernet {
        tx,
        rx: IrqSpinLock::new(rx),
        kick,
        carrier: carrier.clone(),
    });
    let irq = InterruptManager::global();
    let source: Arc<dyn InterruptSource> = Arc::new(Completion {
        device: Arc::downgrade(&device),
        line,
        name: Arc::from(NAME),
    });
    irq.register_interrupt_source(line, source.clone())
        .map_err(|error| error.to_string())?;
    if let Err(error) = irq.enable_external_interrupt(line, cpu) {
        irq.unregister_interrupt_source(line, &source);
        return Err(error.to_string());
    }
    if let Err(error) = slirp::start(library, carrier, host_tx, host_rx, wake) {
        let _ = irq.disable_external_interrupt(line, cpu);
        irq.unregister_interrupt_source(line, &source);
        return Err(error);
    }

    let id = devices.register_device_with_name(NAME.into(), device.clone());
    let interface = Arc::new(EthernetNetworkInterface::new(NAME, device));
    if let Err(error) = manager.register_interface(NAME, interface.clone()) {
        devices.unregister_device(id);
        let _ = irq.disable_external_interrupt(line, cpu);
        irq.unregister_interrupt_source(line, &source);
        return Err(error.into());
    }
    ethernet.register_interface(NAME, mac(), interface);
    ensure_rx_worker_started();
    apply_pending_ip_for_interface(NAME);
    Ok(())
}

fn mac() -> MacAddress {
    MacAddress::new([0x02, 0, 0, 0, 0, 1])
}

struct Carrier {
    cpu: CpuId,
    line: InterruptId,
    up: AtomicBool,
    tx_ready: AtomicBool,
    tx_packets: AtomicU64,
    tx_bytes: AtomicU64,
    tx_errors: AtomicU64,
    rx_packets: AtomicU64,
    rx_bytes: AtomicU64,
    rx_errors: AtomicU64,
    dropped: AtomicU64,
}

impl Carrier {
    fn new(cpu: CpuId, line: InterruptId) -> Self {
        Self {
            cpu,
            line,
            up: AtomicBool::new(true),
            tx_ready: AtomicBool::new(false),
            tx_packets: AtomicU64::new(0),
            tx_bytes: AtomicU64::new(0),
            tx_errors: AtomicU64::new(0),
            rx_packets: AtomicU64::new(0),
            rx_bytes: AtomicU64::new(0),
            rx_errors: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    fn signal(&self) {
        // A failed/retired service can race with IRQ route teardown.
        let _ = scarlet::hosted::raise_external_interrupt(self.cpu, self.line);
    }
}

struct HostedEthernet {
    tx: mpsc::SyncSender<DevicePacket>,
    rx: IrqSpinLock<mpsc::Receiver<DevicePacket>>,
    kick: UnixStream,
    carrier: Arc<Carrier>,
}

struct Completion {
    device: Weak<HostedEthernet>,
    line: InterruptId,
    name: Arc<str>,
}

impl InterruptSource for Completion {
    fn interrupt_id(&self) -> Option<InterruptId> {
        Some(self.line)
    }

    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        let Some(device) = self.device.upgrade() else {
            return Ok(InterruptClaim::NotMine);
        };
        if device.carrier.tx_ready.swap(false, Ordering::AcqRel) {
            notify_transmit_available();
        }
        let packets = device.receive_packets().unwrap_or_default();
        let received = packets.len();
        let enqueued = enqueue_rx_packets(self.name.clone(), packets);
        device
            .carrier
            .dropped
            .fetch_add((received - enqueued) as u64, Ordering::Relaxed);
        Ok(InterruptClaim::Handled)
    }
}

impl Device for HostedEthernet {
    fn name(&self) -> &'static str {
        NAME
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Network
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_network_device(&self) -> Option<&dyn NetworkDevice> {
        Some(self)
    }
}

impl ControlOps for HostedEthernet {}

impl MemoryMappingOps for HostedEthernet {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("Hosted Ethernet uses owned frames")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for HostedEthernet {
    fn wait_until_ready(
        &self,
        _: ReadyInterest,
        _: &mut Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::TimedOut
    }
}

impl NetworkDevice for HostedEthernet {
    fn get_interface_name(&self) -> &'static str {
        NAME
    }

    fn get_mac_address(&self) -> Result<MacAddress, &'static str> {
        Ok(mac())
    }

    fn get_mtu(&self) -> Result<usize, &'static str> {
        Ok(MTU)
    }

    fn get_interface_config(&self) -> Result<NetworkInterfaceConfig, &'static str> {
        Ok(NetworkInterfaceConfig::new(mac(), MTU, NAME))
    }

    fn send_packet(&self, packet: DevicePacket) -> Result<(), &'static str> {
        if !self.is_link_up() {
            return Err("Hosted Ethernet link is down");
        }
        if !(HEADER..=HEADER + MTU).contains(&packet.len) || packet.len > packet.data.len() {
            self.carrier.tx_errors.fetch_add(1, Ordering::Relaxed);
            return Err("Invalid hosted Ethernet frame length");
        }
        // Retain an owned frame without pinning a larger caller allocation
        // behind the queue's frame-count limit.
        let packet = if packet.data.capacity() > HEADER + MTU {
            DevicePacket::with_data(packet.as_slice().to_vec())
        } else {
            packet
        };
        match self.tx.try_send(packet) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => return Err(TX_QUEUE_FULL),
            Err(mpsc::TrySendError::Disconnected(_)) => return Err("Hosted Ethernet link is down"),
        }
        loop {
            match (&self.kick).write(&[1]) {
                Ok(_) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                // A full wake socket is already readable by the host thread.
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(_) => return Err("Hosted Ethernet service stopped"),
            }
        }
    }

    fn receive_packets(&self) -> Result<Vec<DevicePacket>, &'static str> {
        let rx = self.rx.lock();
        let packets: Vec<_> = rx.try_iter().take(QUEUE_CAPACITY).collect();
        if self.is_link_up() {
            Ok(packets)
        } else {
            self.carrier
                .dropped
                .fetch_add(packets.len() as u64, Ordering::Relaxed);
            Ok(Vec::new())
        }
    }

    fn set_promiscuous_mode(&self, enabled: bool) -> Result<(), &'static str> {
        if enabled {
            Err("Promiscuous mode is unavailable on the hosted NAT")
        } else {
            Ok(())
        }
    }

    fn init_network(&mut self) -> Result<(), &'static str> {
        Ok(())
    }

    fn is_link_up(&self) -> bool {
        self.carrier.up.load(Ordering::Acquire)
    }

    fn link_epoch(&self) -> u64 {
        u64::from(!self.is_link_up())
    }

    fn link_observation(&self) -> LinkObservation {
        // This device has one transport lifetime. A new service gets a new
        // device, and a failed service transitions its carrier exactly once.
        let alive = self.is_link_up();
        LinkObservation {
            mac: mac(),
            mtu: MTU as u32,
            kind: self.link_kind(),
            state: if alive {
                scarlet_abi::network::LINK_UP
            } else {
                scarlet_abi::network::LINK_DOWN
            },
            epoch: u64::from(!alive),
        }
    }

    fn get_stats(&self) -> NetworkStats {
        let carrier = &self.carrier;
        NetworkStats {
            tx_packets: carrier.tx_packets.load(Ordering::Relaxed),
            tx_bytes: carrier.tx_bytes.load(Ordering::Relaxed),
            tx_errors: carrier.tx_errors.load(Ordering::Relaxed),
            rx_packets: carrier.rx_packets.load(Ordering::Relaxed),
            rx_bytes: carrier.rx_bytes.load(Ordering::Relaxed),
            rx_errors: carrier.rx_errors.load(Ordering::Relaxed),
            dropped: carrier.dropped.load(Ordering::Relaxed),
        }
    }
}

impl EthernetDevice for HostedEthernet {
    fn mac_address(&self) -> Result<MacAddress, &'static str> {
        self.get_mac_address()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transmit_refuses_a_full_queue_without_discarding_an_accepted_frame() {
        let (tx, host_tx) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (_host_rx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (kick, _wake) = UnixStream::pair().unwrap();
        kick.set_nonblocking(true).unwrap();
        let device = HostedEthernet {
            tx,
            rx: IrqSpinLock::new(rx),
            kick,
            carrier: Arc::new(Carrier::new(0, 19)),
        };
        let mut oversized_backing = Vec::with_capacity(2 * (HEADER + MTU));
        oversized_backing.resize(HEADER, 0);
        device
            .send_packet(DevicePacket::with_data(oversized_backing))
            .unwrap();
        for index in 1..QUEUE_CAPACITY {
            device
                .send_packet(DevicePacket::with_data(vec![index as u8; HEADER]))
                .unwrap();
        }
        assert_eq!(
            device.send_packet(DevicePacket::with_data(vec![0; HEADER])),
            Err(TX_QUEUE_FULL)
        );
        for index in 0..QUEUE_CAPACITY {
            let packet = host_tx.try_recv().unwrap();
            assert_eq!(packet.data, [index as u8; HEADER]);
            assert!(packet.data.capacity() <= HEADER + MTU);
        }
        assert!(matches!(host_tx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}
