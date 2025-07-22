//! Network device interface
//! 
//! This module defines the interface for network devices in the kernel.
//! It provides abstractions for network packet operations and device management.

use core::any::Any;
use alloc::{boxed::Box, vec::Vec};
use spin::Mutex;

use alloc::sync::Arc;

use super::{Device, DeviceType, manager::DeviceManager};
use crate::object::capability::ControlOps;

/// Get the first available network device
/// 
/// This is a convenience function to get the first network device registered in the system.
/// Returns None if no network devices are available.
pub fn get_network_device() -> Option<Arc<dyn Device>> {
    let manager = DeviceManager::get_manager();
    if let Some(device_id) = manager.get_first_device_by_type(DeviceType::Network) {
        return manager.get_device(device_id);
    }
    None
}

/// MAC (Media Access Control) address
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacAddress([u8; 6]);

impl MacAddress {
    /// Create a new MAC address from bytes
    pub const fn new(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }
    
    /// Create a MAC address from a slice (must be exactly 6 bytes)
    pub fn from_slice(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() != 6 {
            return Err("MAC address must be exactly 6 bytes");
        }
        let mut mac = [0u8; 6];
        mac.copy_from_slice(bytes);
        Ok(Self(mac))
    }
    
    /// Get the MAC address as bytes
    pub fn as_bytes(&self) -> &[u8; 6] {
        &self.0
    }
    
    /// Check if this is a broadcast MAC address (FF:FF:FF:FF:FF:FF)
    pub fn is_broadcast(&self) -> bool {
        self.0 == [0xFF; 6]
    }
    
    /// Check if this is a multicast MAC address (first bit of first byte is 1)
    pub fn is_multicast(&self) -> bool {
        (self.0[0] & 0x01) != 0
    }
    
    /// Check if this is a unicast MAC address (not multicast)
    pub fn is_unicast(&self) -> bool {
        !self.is_multicast()
    }
}

/// Device-level network packet for raw data transmission
#[derive(Debug, Clone)]
pub struct DevicePacket {
    /// Raw packet data
    pub data: Vec<u8>,
    /// Length of valid data in the packet
    pub len: usize,
}

impl DevicePacket {
    /// Create a new empty packet
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            len: 0,
        }
    }
    
    /// Create a new packet with the given data
    pub fn with_data(data: Vec<u8>) -> Self {
        let len = data.len();
        Self { data, len }
    }
    
    /// Create a new packet with the given capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            len: 0,
        }
    }
    
    /// Get the packet data as a slice
    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.len]
    }
    
    /// Get the packet data as a mutable slice
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data[..self.len]
    }
    
    /// Set the packet data
    pub fn set_data(&mut self, data: &[u8]) {
        self.data.clear();
        self.data.extend_from_slice(data);
        self.len = data.len();
    }
    
    /// Resize the packet data buffer
    pub fn resize(&mut self, new_len: usize) {
        self.data.resize(new_len, 0);
        self.len = new_len;
    }
}

/// Network interface configuration
#[derive(Debug, Clone)]
pub struct NetworkInterfaceConfig {
    /// MAC address of the interface
    pub mac_address: MacAddress,
    /// Maximum Transmission Unit (MTU) in bytes
    pub mtu: usize,
    /// Interface name
    pub name: &'static str,
    /// Whether the interface supports multicast
    pub multicast_support: bool,
}

impl NetworkInterfaceConfig {
    /// Create a new network interface configuration
    pub fn new(mac_address: MacAddress, mtu: usize, name: &'static str) -> Self {
        Self {
            mac_address,
            mtu,
            name,
            multicast_support: false,
        }
    }
    
    /// Enable multicast support
    pub fn with_multicast(mut self) -> Self {
        self.multicast_support = true;
        self
    }
}

/// Network operation requests
#[derive(Debug)]
pub enum NetworkRequest {
    /// Get interface configuration
    GetInterfaceConfig,
    /// Send a packet
    SendPacket(DevicePacket),
    /// Receive packets (non-blocking)
    ReceivePackets,
    /// Set promiscuous mode
    SetPromiscuous(bool),
}

/// Result of network operations
#[derive(Debug)]
pub struct NetworkResult {
    pub request: Box<NetworkRequest>,
    pub result: Result<NetworkResponse, &'static str>,
}

/// Response from network operations
#[derive(Debug)]
pub enum NetworkResponse {
    /// Interface configuration
    InterfaceConfig(NetworkInterfaceConfig),
    /// Packet sent successfully
    PacketSent,
    /// Received packets
    ReceivedPackets(Vec<DevicePacket>),
    /// Operation completed successfully
    Success,
}

/// Network device interface
/// 
/// This trait defines the interface for network devices.
/// It provides methods for packet transmission, reception, and interface management.
pub trait NetworkDevice: Device {
    /// Get the network interface name
    fn get_interface_name(&self) -> &'static str;
    
    /// Get the MAC address of the interface
    fn get_mac_address(&self) -> Result<MacAddress, &'static str>;
    
    /// Get the MTU (Maximum Transmission Unit) of the interface
    fn get_mtu(&self) -> Result<usize, &'static str>;
    
    /// Get the full interface configuration
    fn get_interface_config(&self) -> Result<NetworkInterfaceConfig, &'static str>;
    
    /// Send a packet
    fn send_packet(&self, packet: DevicePacket) -> Result<(), &'static str>;
    
    /// Receive packets (non-blocking)
    /// Returns all currently available packets
    fn receive_packets(&self) -> Result<Vec<DevicePacket>, &'static str>;
    
    /// Set promiscuous mode (receive all packets on the network)
    fn set_promiscuous_mode(&self, enabled: bool) -> Result<(), &'static str>;
    
    /// Initialize the network device
    fn init_network(&mut self) -> Result<(), &'static str>;
    
    /// Check if the link is up
    fn is_link_up(&self) -> bool;
    
    /// Get network device statistics
    fn get_stats(&self) -> NetworkStats;
}

/// Network device statistics
#[derive(Debug, Clone, Default)]
pub struct NetworkStats {
    /// Number of packets transmitted
    pub tx_packets: u64,
    /// Number of bytes transmitted  
    pub tx_bytes: u64,
    /// Number of transmission errors
    pub tx_errors: u64,
    /// Number of packets received
    pub rx_packets: u64,
    /// Number of bytes received
    pub rx_bytes: u64,
    /// Number of reception errors
    pub rx_errors: u64,
    /// Number of dropped packets
    pub dropped: u64,
}

/// A generic implementation of a network device for testing
pub struct GenericNetworkDevice {
    interface_name: &'static str,
    config: Option<NetworkInterfaceConfig>,
    link_up: bool,
    promiscuous: bool,
    tx_queue: Mutex<Vec<DevicePacket>>,
    rx_queue: Mutex<Vec<DevicePacket>>,
    stats: Mutex<NetworkStats>,
}

impl GenericNetworkDevice {
    /// Create a new generic network device
    pub fn new(interface_name: &'static str) -> Self {
        Self {
            interface_name,
            config: None,
            link_up: false,
            promiscuous: false,
            tx_queue: Mutex::new(Vec::new()),
            rx_queue: Mutex::new(Vec::new()),
            stats: Mutex::new(NetworkStats::default()),
        }
    }
    
    /// Set the interface configuration
    pub fn set_config(&mut self, config: NetworkInterfaceConfig) {
        self.config = Some(config);
    }
    
    /// Set link status
    pub fn set_link_up(&mut self, up: bool) {
        self.link_up = up;
    }
    
    /// Add a packet to the receive queue (for testing)
    pub fn add_received_packet(&self, packet: DevicePacket) {
        self.rx_queue.lock().push(packet);
    }
}

impl Device for GenericNetworkDevice {
    fn device_type(&self) -> super::DeviceType {
        super::DeviceType::Network
    }

    fn name(&self) -> &'static str {
        self.interface_name
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

impl ControlOps for GenericNetworkDevice {
    // Generic network devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl NetworkDevice for GenericNetworkDevice {
    fn get_interface_name(&self) -> &'static str {
        self.interface_name
    }
    
    fn get_mac_address(&self) -> Result<MacAddress, &'static str> {
        self.config.as_ref()
            .map(|config| config.mac_address)
            .ok_or("Interface not configured")
    }
    
    fn get_mtu(&self) -> Result<usize, &'static str> {
        self.config.as_ref()
            .map(|config| config.mtu)
            .ok_or("Interface not configured")
    }
    
    fn get_interface_config(&self) -> Result<NetworkInterfaceConfig, &'static str> {
        self.config.clone().ok_or("Interface not configured")
    }
    
    fn send_packet(&self, packet: DevicePacket) -> Result<(), &'static str> {
        if !self.link_up {
            return Err("Link is down");
        }
        
        // Update statistics
        {
            let mut stats = self.stats.lock();
            stats.tx_packets += 1;
            stats.tx_bytes += packet.len as u64;
        }
        
        // In a real implementation, this would send the packet to hardware
        // For testing, we just add it to the tx queue
        self.tx_queue.lock().push(packet);
        Ok(())
    }
    
    fn receive_packets(&self) -> Result<Vec<DevicePacket>, &'static str> {
        if !self.link_up {
            return Ok(Vec::new());
        }
        
        let packets = {
            let mut rx_queue = self.rx_queue.lock();
            core::mem::replace(&mut *rx_queue, Vec::new())
        };
        
        // Update statistics
        {
            let mut stats = self.stats.lock();
            stats.rx_packets += packets.len() as u64;
            stats.rx_bytes += packets.iter().map(|p| p.len as u64).sum::<u64>();
        }
        
        Ok(packets)
    }
    
    fn set_promiscuous_mode(&self, enabled: bool) -> Result<(), &'static str> {
        // In a real implementation, this would configure hardware
        // For testing, we just update the flag
        // Note: This would need interior mutability in a real implementation
        Ok(())
    }
    
    fn init_network(&mut self) -> Result<(), &'static str> {
        // Generic initialization - set default config if none exists
        if self.config.is_none() {
            let default_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
            let config = NetworkInterfaceConfig::new(default_mac, 1500, self.interface_name);
            self.config = Some(config);
        }
        self.link_up = true;
        Ok(())
    }
    
    fn is_link_up(&self) -> bool {
        self.link_up
    }
    
    fn get_stats(&self) -> NetworkStats {
        self.stats.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceType;
    use alloc::vec;

    #[test_case]
    fn test_mac_address() {
        let mac = MacAddress::new([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        assert_eq!(mac.as_bytes(), &[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        assert!(!mac.is_broadcast());
        assert!(!mac.is_multicast());
        assert!(mac.is_unicast());
        
        let broadcast = MacAddress::new([0xFF; 6]);
        assert!(broadcast.is_broadcast());
        assert!(broadcast.is_multicast());
        assert!(!broadcast.is_unicast());
        
        let multicast = MacAddress::new([0x01, 0x00, 0x5e, 0x00, 0x00, 0x01]);
        assert!(!multicast.is_broadcast());
        assert!(multicast.is_multicast());
        assert!(!multicast.is_unicast());
    }

    #[test_case]
    fn test_mac_address_from_slice() {
        let bytes = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let mac = MacAddress::from_slice(&bytes).unwrap();
        assert_eq!(mac.as_bytes(), &bytes);
        
        // Test invalid length
        let invalid = [0x00, 0x11, 0x22];
        assert!(MacAddress::from_slice(&invalid).is_err());
    }

    #[test_case]
    fn test_network_packet() {
        let mut packet = DevicePacket::new();
        assert_eq!(packet.len, 0);
        assert_eq!(packet.as_slice().len(), 0);
        
        let data = vec![0x00, 0x11, 0x22, 0x33];
        packet.set_data(&data);
        assert_eq!(packet.len, 4);
        assert_eq!(packet.as_slice(), &data);
        
        let packet2 = DevicePacket::with_data(data.clone());
        assert_eq!(packet2.len, 4);
        assert_eq!(packet2.as_slice(), &data);
        
        let mut packet3 = DevicePacket::with_capacity(10);
        packet3.resize(6);
        assert_eq!(packet3.len, 6);
        assert_eq!(packet3.data.len(), 6);
    }

    #[test_case]
    fn test_interface_config() {
        let mac = MacAddress::new([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        let config = NetworkInterfaceConfig::new(mac, 1500, "eth0");
        assert_eq!(config.mac_address, mac);
        assert_eq!(config.mtu, 1500);
        assert_eq!(config.name, "eth0");
        assert!(!config.multicast_support);
        
        let config_mc = config.with_multicast();
        assert!(config_mc.multicast_support);
    }

    #[test_case]
    fn test_generic_network_device() {
        let mut device = GenericNetworkDevice::new("test0");
        assert_eq!(device.get_interface_name(), "test0");
        assert_eq!(device.device_type(), DeviceType::Network);
        assert!(!device.is_link_up());
        
        // Test initialization
        device.init_network().unwrap();
        assert!(device.is_link_up());
        assert!(device.get_mac_address().is_ok());
        assert!(device.get_mtu().is_ok());
        assert!(device.get_interface_config().is_ok());
        
        // Test packet operations
        let test_data = vec![0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let packet = DevicePacket::with_data(test_data);
        assert!(device.send_packet(packet).is_ok());
        
        // Check statistics
        let stats = device.get_stats();
        assert_eq!(stats.tx_packets, 1);
        assert_eq!(stats.tx_bytes, 6);
        
        // Test receive
        let rx_packet = DevicePacket::with_data(vec![0xAA, 0xBB, 0xCC]);
        device.add_received_packet(rx_packet);
        let received = device.receive_packets().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].as_slice(), &[0xAA, 0xBB, 0xCC]);
        
        // Check updated statistics
        let stats = device.get_stats();
        assert_eq!(stats.rx_packets, 1);
        assert_eq!(stats.rx_bytes, 3);
    }

    #[test_case]
    fn test_link_down_behavior() {
        let mut device = GenericNetworkDevice::new("test0");
        device.init_network().unwrap();
        device.set_link_up(false);
        
        let packet = DevicePacket::with_data(vec![0x01, 0x02, 0x03]);
        assert!(device.send_packet(packet).is_err());
        
        let received = device.receive_packets().unwrap();
        assert_eq!(received.len(), 0);
    }

    #[test_case]
    fn test_network_stats() {
        let mut device = GenericNetworkDevice::new("test0");
        device.init_network().unwrap();
        
        // Send multiple packets
        for i in 0..5 {
            let data = vec![i; (i + 1) as usize];
            let packet = DevicePacket::with_data(data);
            device.send_packet(packet).unwrap();
        }
        
        let stats = device.get_stats();
        assert_eq!(stats.tx_packets, 5);
        assert_eq!(stats.tx_bytes, 1 + 2 + 3 + 4 + 5); // Sum of packet sizes
    }

    #[test_case]
    fn test_get_network_device_none() {
        // Test when no network devices are registered
        // Note: This test assumes no network devices are registered in the test environment
        let result = get_network_device();
        // We can't assert the exact result since it depends on test environment state
        // But we can ensure the function doesn't panic and returns the correct type
        match result {
            Some(device) => {
                // If a device is found, it should be a network device
                assert_eq!(device.device_type(), DeviceType::Network);
            },
            None => {
                // No network device found - this is expected in test environment
            }
        }
    }
}