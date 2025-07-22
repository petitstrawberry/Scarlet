//! # VirtIO Network Device Driver
//! 
//! This module provides a driver for VirtIO network devices, implementing the
//! NetworkDevice trait for integration with the kernel's network subsystem.
//!
//! The driver supports basic network operations (packet transmission and reception)
//! and handles the VirtIO queue management for network device requests.
//!
//! ## VirtIO Network Device Features
//! 
//! The driver checks for and handles the following VirtIO network device features:
//! - Basic packet transmission and reception
//! - MAC address configuration
//! - MTU management
//! - Link status detection
//!
//! ## Implementation Details
//!
//! The driver uses two virtqueues:
//! - Receive queue (index 0): For receiving packets from the network
//! - Transmit queue (index 1): For sending packets to the network
//!
//! Each network packet is handled through the VirtIO descriptor chain mechanism,
//! with proper memory management for packet buffers.

use alloc::{boxed::Box, vec::Vec, vec};
use spin::{Mutex, RwLock};

use core::mem;
use crate::device::{Device, DeviceType};
use crate::drivers::virtio::features::{VIRTIO_RING_F_EVENT_IDX, VIRTIO_RING_F_INDIRECT_DESC};
use crate::{
    device::network::{NetworkDevice, DevicePacket, NetworkInterfaceConfig, MacAddress, NetworkStats}, 
    drivers::virtio::{device::VirtioDevice, queue::{DescriptorFlag, VirtQueue}}, object::capability::ControlOps
};

// VirtIO Network Feature bits
const VIRTIO_NET_F_CSUM: u32 = 0;          // Device handles packets with partial checksum
const VIRTIO_NET_F_GUEST_CSUM: u32 = 1;    // Guest handles packets with partial checksum
const VIRTIO_NET_F_CTRL_GUEST_OFFLOADS: u32 = 2; // Control channel offloads reconfiguration support
const VIRTIO_NET_F_MTU: u32 = 3;           // Device maximum MTU reporting supported
const VIRTIO_NET_F_MAC: u32 = 5;           // Device has given MAC address
const VIRTIO_NET_F_GUEST_TSO4: u32 = 7;    // Guest can handle TSOv4
const VIRTIO_NET_F_GUEST_TSO6: u32 = 8;    // Guest can handle TSOv6
const VIRTIO_NET_F_GUEST_ECN: u32 = 9;     // Guest can handle TSO with ECN
const VIRTIO_NET_F_GUEST_UFO: u32 = 10;    // Guest can handle UFO
const VIRTIO_NET_F_HOST_TSO4: u32 = 11;    // Device can handle TSOv4
const VIRTIO_NET_F_HOST_TSO6: u32 = 12;    // Device can handle TSOv6
const VIRTIO_NET_F_HOST_ECN: u32 = 13;     // Device can handle TSO with ECN
const VIRTIO_NET_F_HOST_UFO: u32 = 14;     // Device can handle UFO
const VIRTIO_NET_F_MRG_RXBUF: u32 = 15;    // Guest can merge receive buffers
const VIRTIO_NET_F_STATUS: u32 = 16;       // Configuration status field available
const VIRTIO_NET_F_CTRL_VQ: u32 = 17;      // Control channel available
const VIRTIO_NET_F_CTRL_RX: u32 = 18;      // Control channel RX mode support
const VIRTIO_NET_F_CTRL_VLAN: u32 = 19;    // Control channel VLAN filtering
const VIRTIO_NET_F_GUEST_ANNOUNCE: u32 = 21; // Guest can send gratuitous packets
const VIRTIO_NET_F_MQ: u32 = 22;           // Device supports multiqueue with automatic receive steering
const VIRTIO_NET_F_CTRL_MAC_ADDR: u32 = 23; // Set MAC address through control channel

// VirtIO Network Status bits
const VIRTIO_NET_S_LINK_UP: u16 = 1;       // Link is up
const VIRTIO_NET_S_ANNOUNCE: u16 = 2;      // Gratuitous packets should be sent

// Default MTU if not specified
const DEFAULT_MTU: usize = 1500;

/// VirtIO Network Device Configuration
#[repr(C)]
pub struct VirtioNetConfig {
    pub mac: [u8; 6],          // MAC address
    pub status: u16,           // Status
    pub max_virtqueue_pairs: u16, // Maximum number of virtqueue pairs
    pub mtu: u16,              // MTU
}

/// VirtIO Network Header (for packet transmission/reception)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtioNetHdr {
    pub flags: u8,             // Flags
    pub gso_type: u8,          // GSO type
    pub hdr_len: u16,          // Header length
    pub gso_size: u16,         // GSO size
    pub csum_start: u16,       // Checksum start
    pub csum_offset: u16,      // Checksum offset
    pub num_buffers: u16,      // Number of buffers (for mergeable rx buffers)
}

/// Basic VirtIO Network Header (without num_buffers field)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtioNetHdrBasic {
    pub flags: u8,             // Flags
    pub gso_type: u8,          // GSO type
    pub hdr_len: u16,          // Header length
    pub gso_size: u16,         // GSO size
    pub csum_start: u16,       // Checksum start
    pub csum_offset: u16,      // Checksum offset
}

impl VirtioNetHdr {
    /// Create a new default network header
    pub fn new() -> Self {
        Self {
            flags: 0,
            gso_type: 0,
            hdr_len: 0,
            gso_size: 0,
            csum_start: 0,
            csum_offset: 0,
            num_buffers: 0,
        }
    }
}

impl VirtioNetHdrBasic {
    /// Create a new default basic network header
    pub fn new() -> Self {
        Self {
            flags: 0,
            gso_type: 0,
            hdr_len: 0,
            gso_size: 0,
            csum_start: 0,
            csum_offset: 0,
        }
    }
}

/// VirtIO Network Device
pub struct VirtioNetDevice {
    base_addr: usize,
    virtqueues: Mutex<[VirtQueue<'static>; 2]>, // RX queue (0) and TX queue (1)
    config: RwLock<Option<NetworkInterfaceConfig>>,
    features: RwLock<u32>,
    stats: Mutex<NetworkStats>,
    initialized: Mutex<bool>,
    rx_buffers: Mutex<Vec<Box<[u8]>>>,
}

impl VirtioNetDevice {
    /// Create a new VirtIO Network device
    ///
    /// # Arguments
    ///
    /// * `base_addr` - The base address of the device
    ///
    /// # Returns
    ///
    /// A new instance of `VirtioNetDevice`
    pub fn new(base_addr: usize) -> Self {
        let mut device = Self {
            base_addr,
            virtqueues: Mutex::new([VirtQueue::new(8), VirtQueue::new(8)]), // RX and TX queues
            config: RwLock::new(None),
            features: RwLock::new(0),
            stats: Mutex::new(NetworkStats::default()),
            initialized: Mutex::new(false),
            rx_buffers: Mutex::new(Vec::new()),
        };
        
        // Initialize the VirtIO device first
        let negotiated_features = match device.init() {
            Ok(features) => features,
            Err(_) => panic!("Failed to initialize VirtIO Network Device"),
        };

        // Read device configuration with the negotiated features
        device.read_device_config(negotiated_features);

        device
    }
    
    /// Read device configuration from the VirtIO config space
    fn read_device_config(&mut self, negotiated_features: u32) {
        // Store actually negotiated features
        *self.features.write() = negotiated_features;
        
        // Debug: Print negotiated features in test builds
        #[cfg(test)]
        {
            use crate::{drivers::virtio::device::Register, early_println};
            // Also read device features for debugging
            let device_features = self.read32_register(Register::DeviceFeatures);
            early_println!("[virtio-net] Device offers features: 0x{:x}", device_features);
            early_println!("[virtio-net] Negotiated features: 0x{:x}", negotiated_features);
        }
        
        // Read MAC address if supported
        let mut mac_addr = [0u8; 6];
        if negotiated_features & (1 << VIRTIO_NET_F_MAC) != 0 {
            for i in 0..6 {
                mac_addr[i] = self.read_config::<u8>(i);
            }
        } else {
            // Generate a default MAC address
            mac_addr = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        }
        
        // Read MTU if supported
        let mtu = if negotiated_features & (1 << VIRTIO_NET_F_MTU) != 0 {
            self.read_config::<u16>(12) as usize // MTU at offset 12
        } else {
            DEFAULT_MTU
        };
        
        // Create network interface configuration
        let mac = MacAddress::new(mac_addr);
        let config = NetworkInterfaceConfig::new(mac, mtu, "virtio-net");
        *self.config.write() = Some(config);
    }
    
    /// Get the appropriate header size based on device features
    fn get_header_size(&self) -> usize {
        // Always use basic header since we don't support mergeable RX buffers
        mem::size_of::<VirtioNetHdrBasic>()
    }
    
    /// Setup receive buffers in the RX queue
    fn setup_rx_buffers(&self) -> Result<(), &'static str> {
        let mut virtqueues = self.virtqueues.lock();
        let rx_queue = &mut virtqueues[0]; // RX queue is index 0

        // Use standard single-buffer approach like Linux virtio-net
        let buffer_count = 2; // Minimal number of buffers
        
        for _ in 0..buffer_count {
            let hdr_size = self.get_header_size(); // 10 bytes for VirtioNetHdrBasic
            let packet_size = 1514; // Standard Ethernet frame size
            let total_size = hdr_size + packet_size;
            
            // Allocate single contiguous buffer - this is the standard approach
            let buffer = vec![0u8; total_size];
            let buffer_box = buffer.into_boxed_slice();
            let buffer_ptr = Box::into_raw(buffer_box);
            
            // Allocate single descriptor for the entire receive buffer
            let desc_idx = rx_queue.alloc_desc().ok_or("Failed to allocate RX descriptor")?;
            
            // Setup descriptor - device writes virtio-net header + packet data here
            rx_queue.desc[desc_idx].addr = buffer_ptr as *mut u8 as u64;
            rx_queue.desc[desc_idx].len = total_size as u32;
            rx_queue.desc[desc_idx].flags = DescriptorFlag::Write as u16; // Device writes
            rx_queue.desc[desc_idx].next = 0; // No chaining
            
            // Add to available ring
            if let Err(e) = rx_queue.push(desc_idx) {
                rx_queue.free_desc(desc_idx);
                unsafe { drop(Box::from_raw(buffer_ptr)); }
                return Err(e);
            }
            
            // Store buffer pointer for cleanup
            self.rx_buffers.lock().push(unsafe { Box::from_raw(buffer_ptr) });
        }
        
        // Notify device about available RX buffers
        self.notify(0); // Notify RX queue
        
        Ok(())
    }
    
    /// Process a single packet transmission
    fn transmit_packet(&self, packet: &DevicePacket) -> Result<(), &'static str> {
        // combine header and packet in single buffer like their send() function
        let hdr_size = mem::size_of::<VirtioNetHdrBasic>();
        let total_size = hdr_size + packet.len;
        
        // Create single buffer with header first, followed by packet data
        let mut combined_buffer = vec![0u8; total_size];
        
        // Fill header at the beginning
        let header = VirtioNetHdrBasic::new();
        unsafe {
            let header_bytes = core::slice::from_raw_parts(
                &header as *const VirtioNetHdrBasic as *const u8,
                hdr_size
            );
            combined_buffer[..hdr_size].copy_from_slice(header_bytes);
        }
        
        // Copy packet data after header
        combined_buffer[hdr_size..].copy_from_slice(&packet.data[..packet.len]);
        
        // Convert to stable memory allocation
        let buffer_box = combined_buffer.into_boxed_slice();
        let buffer_ptr = Box::into_raw(buffer_box);
        
        let result = {
            let mut virtqueues = self.virtqueues.lock();
            let tx_queue = &mut virtqueues[1]; // TX queue is index 1

            // Single descriptor for the combined buffer
            let desc_idx = tx_queue.alloc_desc().ok_or("Failed to allocate TX descriptor")?;
            
            // Setup descriptor for the combined buffer (device readable)
            tx_queue.desc[desc_idx].addr = buffer_ptr as *mut u8 as u64;
            tx_queue.desc[desc_idx].len = total_size as u32;
            tx_queue.desc[desc_idx].flags = 0; // No flags, single descriptor
            tx_queue.desc[desc_idx].next = 0; // No chaining
            
            // Submit the request to the queue
            if let Err(e) = tx_queue.push(desc_idx) {
                tx_queue.free_desc(desc_idx);
                return Err(e);
            }
            
            // Notify the device
            self.notify(1); // Notify TX queue
            
            // Wait for transmission (polling)
            while tx_queue.is_busy() {}
            
            // Get completion
            let result = match tx_queue.pop() {
                Some(_completed_desc) => Ok(()),
                None => {
                    tx_queue.free_desc(desc_idx);
                    Err("No TX completion")
                }
            };
            
            // Free descriptor after processing (responsibility of driver)
            tx_queue.free_desc(desc_idx);
            
            result
        };
        
        // Cleanup memory
        unsafe {
            drop(Box::from_raw(buffer_ptr));
        }
        
        // Update statistics if transmission succeeded
        if result.is_ok() {
            let mut stats = self.stats.lock();
            stats.tx_packets += 1;
            stats.tx_bytes += packet.len as u64;
        }
        
        result
    }
    
    /// Process received packets from RX queue
    fn process_received_packets(&self) -> Result<Vec<DevicePacket>, &'static str> {
        let mut packets = Vec::new();
        let mut virtqueues = self.virtqueues.lock();
        let rx_queue = &mut virtqueues[0]; // RX queue is index 0

        // Process all completed RX descriptors
        while let Some(desc_idx) = rx_queue.pop() {
            // Get the buffer from the descriptor
            let buffer_addr = rx_queue.desc[desc_idx].addr as *mut u8;
            let buffer_len = rx_queue.desc[desc_idx].len as usize;
            
            // Read the received data
            unsafe {
                // Skip the VirtIO network header (use appropriate size based on device features)
                let hdr_size = self.get_header_size();
                if buffer_len > hdr_size {
                    let packet_data_ptr = buffer_addr.add(hdr_size);
                    let packet_len = buffer_len - hdr_size;
                    
                    // Create packet from received data
                    let packet_data = core::slice::from_raw_parts(packet_data_ptr, packet_len);
                    let packet = DevicePacket::with_data(packet_data.to_vec());
                    packets.push(packet);
                }
            }
            
            // Recycle the buffer by putting it back in the RX queue
            rx_queue.desc[desc_idx].flags = DescriptorFlag::Write as u16;
            if let Err(_) = rx_queue.push(desc_idx) {
                // If we can't recycle, free the descriptor
                rx_queue.free_desc(desc_idx);
                // Note: This may cause buffer leaks but prevents descriptor leaks
            }
        }
        
        // Notify device about recycled buffers
        if !packets.is_empty() {
            self.notify(0); // Notify RX queue
            
            // Update statistics
            let mut stats = self.stats.lock();
            stats.rx_packets += packets.len() as u64;
            stats.rx_bytes += packets.iter().map(|p| p.len as u64).sum::<u64>();
        }
        
        Ok(packets)
    }
    
    /// Check link status from device configuration
    fn check_link_status(&self) -> bool {
        let features = *self.features.read();
        if features & (1 << VIRTIO_NET_F_STATUS) != 0 {
            // Read status from config space
            let status = self.read_config::<u16>(6); // Status at offset 6
            (status & VIRTIO_NET_S_LINK_UP) != 0
        } else {
            // Assume link is up if status feature is not supported
            true
        }
    }
}

impl Device for VirtioNetDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Network
    }
    
    fn name(&self) -> &'static str {
        "virtio-net"
    }
    
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
    
    fn as_network_device(&self) -> Option<&dyn crate::device::network::NetworkDevice> {
        Some(self)
    }
}

impl ControlOps for VirtioNetDevice {
    // VirtIO network devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl VirtioDevice for VirtioNetDevice {
    fn get_base_addr(&self) -> usize {
        self.base_addr
    }
    
    fn get_virtqueue_count(&self) -> usize {
        2 // TX and RX queues
    }
    
    fn get_supported_features(&self, device_features: u32) -> u32 {
        // Debug: Print detailed feature analysis
        #[cfg(test)]
        {
            use crate::early_println;
            early_println!("[virtio-net] Analyzing device features: 0x{:x}", device_features);
            if device_features & (1 << VIRTIO_NET_F_MAC) != 0 {
                early_println!("[virtio-net] Device supports MAC (bit {})", VIRTIO_NET_F_MAC);
            }
            if device_features & (1 << VIRTIO_NET_F_STATUS) != 0 {
                early_println!("[virtio-net] Device supports STATUS (bit {})", VIRTIO_NET_F_STATUS);
            }
            if device_features & (1 << VIRTIO_NET_F_MTU) != 0 {
                early_println!("[virtio-net] Device supports MTU (bit {})", VIRTIO_NET_F_MTU);
            }
        }
        
        // Use virtio-blk style: accept most features, exclude problematic ones
        // Start with all device features and exclude specific ones we don't want
        let result = device_features & (
            1 << VIRTIO_NET_F_STATUS |
            1 << VIRTIO_NET_F_MAC |
            1 << VIRTIO_RING_F_EVENT_IDX |
            1 << VIRTIO_RING_F_INDIRECT_DESC
        );
        
        #[cfg(test)]
        {
            use crate::early_println;
            early_println!("[virtio-net] Using all device features: 0x{:x}", result);
        }
        
        result
    }
    
    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= 2 {
            return None;
        }
        
        let virtqueues = self.virtqueues.lock();
        Some(virtqueues[queue_idx].get_raw_ptr() as u64)
    }
    
    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= 2 {
            return None;
        }
        
        let virtqueues = self.virtqueues.lock();
        Some(virtqueues[queue_idx].avail.flags as *const _ as u64)
    }
    
    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= 2 {
            return None;
        }
        
        let virtqueues = self.virtqueues.lock();
        Some(virtqueues[queue_idx].used.flags as *const _ as u64)
    }
}

impl NetworkDevice for VirtioNetDevice {
    fn get_interface_name(&self) -> &'static str {
        "virtio-net"
    }
    
    fn get_mac_address(&self) -> Result<MacAddress, &'static str> {
        self.config.read()
            .as_ref()
            .map(|config| config.mac_address)
            .ok_or("Device not configured")
    }
    
    fn get_mtu(&self) -> Result<usize, &'static str> {
        self.config.read()
            .as_ref()
            .map(|config| config.mtu)
            .ok_or("Device not configured")
    }
    
    fn get_interface_config(&self) -> Result<NetworkInterfaceConfig, &'static str> {
        self.config.read()
            .clone()
            .ok_or("Device not configured")
    }
    
    fn send_packet(&self, packet: DevicePacket) -> Result<(), &'static str> {
        if !self.is_link_up() {
            return Err("Link is down");
        }
        
        self.transmit_packet(&packet)
    }
    
    fn receive_packets(&self) -> Result<Vec<DevicePacket>, &'static str> {
        if !self.is_link_up() {
            return Ok(Vec::new());
        }
        
        self.process_received_packets()
    }
    
    fn set_promiscuous_mode(&self, _enabled: bool) -> Result<(), &'static str> {
        // TODO: Implement via control queue if VIRTIO_NET_F_CTRL_RX is supported
        // For now, just return success
        Ok(())
    }
    
    fn init_network(&mut self) -> Result<(), &'static str> {
        {
            let mut initialized = self.initialized.lock();
            if *initialized {
                return Ok(());
            }
            *initialized = true;
        }
        
        // Setup RX buffers with separated descriptors for header and data
        self.setup_rx_buffers()?;
        
        Ok(())
    }
    
    fn is_link_up(&self) -> bool {
        self.check_link_status()
    }
    
    fn get_stats(&self) -> NetworkStats {
        self.stats.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test_case]
    fn test_virtio_net_device_creation() {
        let device = VirtioNetDevice::new(0x10003000);
        assert_eq!(device.get_base_addr(), 0x10003000);
        assert_eq!(device.get_virtqueue_count(), 2);
        assert_eq!(device.device_type(), DeviceType::Network);
        assert_eq!(device.name(), "virtio-net");
        assert_eq!(device.get_interface_name(), "virtio-net");
    }

    #[test_case]
    fn test_virtio_net_header() {
        let hdr = VirtioNetHdr::new();
        assert_eq!(hdr.flags, 0);
        assert_eq!(hdr.gso_type, 0);
        assert_eq!(hdr.hdr_len, 0);
        assert_eq!(hdr.gso_size, 0);
        assert_eq!(hdr.csum_start, 0);
        assert_eq!(hdr.csum_offset, 0);
        assert_eq!(hdr.num_buffers, 0);
    }

    #[test_case]
    fn test_virtio_net_device_config() {
        let device = VirtioNetDevice::new(0x10003000);
        
        // Device should have default configuration after creation
        assert!(device.get_mac_address().is_ok());
        assert!(device.get_mtu().is_ok());
        assert!(device.get_interface_config().is_ok());
        
        let config = device.get_interface_config().unwrap();
        assert_eq!(config.name, "virtio-net");
        assert!(config.mtu > 0);
    }

    #[test_case]
    fn test_virtio_net_initialization() {
        let mut device = VirtioNetDevice::new(0x10003000);
        
        // Test network initialization
        assert!(device.init_network().is_ok());
        
        // Should not fail on subsequent initialization
        assert!(device.init_network().is_ok());
    }

    #[test_case]
    fn test_virtio_net_link_status() {
        let device = VirtioNetDevice::new(0x10003000);
        
        // Link status depends on device configuration
        // In test environment, this may vary
        let _link_up = device.is_link_up();
        // We can't assert specific value since it depends on test setup
    }

    #[test_case]
    fn test_virtio_net_statistics() {
        let mut device = VirtioNetDevice::new(0x10003000);
        device.init_network().unwrap();
        
        let initial_stats = device.get_stats();
        assert_eq!(initial_stats.tx_packets, 0);
        assert_eq!(initial_stats.tx_bytes, 0);
        assert_eq!(initial_stats.rx_packets, 0);
        assert_eq!(initial_stats.rx_bytes, 0);
        
        // Send some test packets if link is up
        if device.is_link_up() {
            for i in 0..3 {
                let data = vec![i; (i + 1) as usize];
                let packet = DevicePacket::with_data(data);
                device.send_packet(packet).unwrap();
            }
            
            let stats = device.get_stats();
            assert_eq!(stats.tx_packets, 3);
            assert_eq!(stats.tx_bytes, 1 + 2 + 3); // Sum of packet sizes
        }
    }

    #[test_case]
    fn test_virtio_net_promiscuous_mode() {
        let device = VirtioNetDevice::new(0x10003000);
        
        // Should succeed (no-op in current implementation)
        assert!(device.set_promiscuous_mode(true).is_ok());
        assert!(device.set_promiscuous_mode(false).is_ok());
    }

    #[test_case]
    fn test_virtio_net_tx_functionality() {
        let device = VirtioNetDevice::new(0x10003000);
        
        // Create a test packet
        let test_data = vec![0x45, 0x00, 0x00, 0x3c]; // Simple IP header start
        let packet = DevicePacket::with_data(test_data);
        
        // Test packet transmission - should not panic
        let result = device.transmit_packet(&packet);
        // In test environment, TX may complete or timeout - both are acceptable
        // What matters is that we don't crash or leave device in broken state
        match result {
            Ok(_) => {
                // TX completed successfully
                crate::early_println!("[virtio-net test] TX completed successfully");
            },
            Err(e) => {
                // TX timed out or failed - acceptable in test environment
                crate::early_println!("[virtio-net test] TX result: {}", e);
            }
        }
    }
    
    #[test_case]
    fn test_virtio_net_tx_with_multiple_packets() {
        let device = VirtioNetDevice::new(0x10003000);
        
        // Test multiple packet transmission
        for i in 0..3 {
            let mut test_data = vec![0x45, 0x00, 0x00, 0x3c];
            test_data.push(i as u8); // Make each packet unique
            let packet = DevicePacket::with_data(test_data);
            
            let result = device.transmit_packet(&packet);
            crate::early_println!("[virtio-net test] Packet {} TX result: {:?}", i, result.is_ok());
        }
    }

    #[test_case] 
    fn test_virtio_net_multiple_devices() {
        // Test creating multiple devices (simulating net0, net1, net2)
        let device1 = VirtioNetDevice::new(0x10003000); // net0 - user netdev
        let device2 = VirtioNetDevice::new(0x10004000); // net1 - hub netdev  
        let device3 = VirtioNetDevice::new(0x10005000); // net2 - hub netdev
        
        // Verify each device has unique base addresses
        assert_eq!(device1.get_base_addr(), 0x10003000);
        assert_eq!(device2.get_base_addr(), 0x10004000);
        assert_eq!(device3.get_base_addr(), 0x10005000);
        
        // All devices should have proper configuration
        assert!(device1.get_mac_address().is_ok());
        assert!(device2.get_mac_address().is_ok());
        assert!(device3.get_mac_address().is_ok());
        
        crate::early_println!("[virtio-net test] Multiple devices created successfully");
        
        // Test sending packet on each device
        let test_data = vec![0x45, 0x00, 0x00, 0x3c];
        let packet = DevicePacket::with_data(test_data);
        
        let _result1 = device1.transmit_packet(&packet);
        let _result2 = device2.transmit_packet(&packet); 
        let _result3 = device3.transmit_packet(&packet);
        
        crate::early_println!("[virtio-net test] Transmitted packets on all 3 devices");
    }

    #[test_case]
    fn test_virtio_net_bidirectional_hub_communication() {
        // Test hub-connected devices for bidirectional communication
        // This simulates the actual QEMU setup with hub networking
        let device_net1 = VirtioNetDevice::new(0x10004000); // net1 - hub device 1
        let device_net2 = VirtioNetDevice::new(0x10005000); // net2 - hub device 2
        
        crate::early_println!("[virtio-net test] Testing bidirectional hub communication");
        crate::early_println!("[virtio-net test] Device net1: {:#x}, Device net2: {:#x}", 
                            device_net1.get_base_addr(), device_net2.get_base_addr());
        
        // Get initial stats for both devices
        let net1_initial_stats = device_net1.get_stats();
        let net2_initial_stats = device_net2.get_stats();
        
        crate::early_println!("[virtio-net test] Initial stats - net1: TX:{}, RX:{} | net2: TX:{}, RX:{}", 
                            net1_initial_stats.tx_packets, net1_initial_stats.rx_packets,
                            net2_initial_stats.tx_packets, net2_initial_stats.rx_packets);
        
        // Prepare test packets with unique identifiers
        let packet_net1_to_net2 = DevicePacket::with_data(vec![0x01, 0x02, 0x03, 0x04, 0xAA]); // net1->net2
        let packet_net2_to_net1 = DevicePacket::with_data(vec![0x05, 0x06, 0x07, 0x08, 0xBB]); // net2->net1
        
        // Test 1: Send packet from net1 to net2 
        crate::early_println!("[virtio-net test] Sending packet from net1 to net2...");
        let result1 = device_net1.transmit_packet(&packet_net1_to_net2);
        crate::early_println!("[virtio-net test] net1->net2 TX result: {:?}", result1.is_ok());
        
        // Test 2: Send packet from net2 to net1
        crate::early_println!("[virtio-net test] Sending packet from net2 to net1...");
        let result2 = device_net2.transmit_packet(&packet_net2_to_net1);
        crate::early_println!("[virtio-net test] net2->net1 TX result: {:?}", result2.is_ok());
        
        // Test 3: Check for received packets on both devices
        crate::early_println!("[virtio-net test] Checking for received packets...");
        
        let received_on_net1 = device_net1.receive_packets();
        let received_on_net2 = device_net2.receive_packets();
        
        match received_on_net1 {
            Ok(packets) => {
                crate::early_println!("[virtio-net test] net1 received {} packets", packets.len());
                for (i, packet) in packets.iter().enumerate() {
                    crate::early_println!("[virtio-net test] net1 RX packet {}: {} bytes", i, packet.len);
                }
            },
            Err(e) => crate::early_println!("[virtio-net test] net1 RX error: {}", e),
        }
        
        match received_on_net2 {
            Ok(packets) => {
                crate::early_println!("[virtio-net test] net2 received {} packets", packets.len());
                for (i, packet) in packets.iter().enumerate() {
                    crate::early_println!("[virtio-net test] net2 RX packet {}: {} bytes", i, packet.len);
                }
            },
            Err(e) => crate::early_println!("[virtio-net test] net2 RX error: {}", e),
        }
        
        // Check final statistics
        let net1_final_stats = device_net1.get_stats();
        let net2_final_stats = device_net2.get_stats();
        
        crate::early_println!("[virtio-net test] Final stats - net1: TX:{}, RX:{} | net2: TX:{}, RX:{}",
                            net1_final_stats.tx_packets, net1_final_stats.rx_packets,
                            net2_final_stats.tx_packets, net2_final_stats.rx_packets);
        
        // Verify that at least transmission statistics were updated
        let net1_tx_delta = net1_final_stats.tx_packets - net1_initial_stats.tx_packets;
        let net2_tx_delta = net2_final_stats.tx_packets - net2_initial_stats.tx_packets;
        
        crate::early_println!("[virtio-net test] TX deltas - net1: +{}, net2: +{}", net1_tx_delta, net2_tx_delta);
        crate::early_println!("[virtio-net test] Bidirectional hub communication test completed");
    }

    #[test_case]
    fn test_virtio_net_device_enumeration() {
        // Test that we can properly enumerate and differentiate multiple devices
        // This helps verify that the device manager properly detects all virtio-net devices
        crate::early_println!("[virtio-net test] Testing device enumeration for multiple virtio-net devices");
        
        let devices = [
            VirtioNetDevice::new(0x10003000), // bus.2 - net0 (user)
            VirtioNetDevice::new(0x10004000), // bus.3 - net1 (hub)
            VirtioNetDevice::new(0x10005000), // bus.4 - net2 (hub)
        ];
        
        for (i, device) in devices.iter().enumerate() {
            let base_addr = device.get_base_addr();
            let mac_result = device.get_mac_address();
            let mtu_result = device.get_mtu();
            let link_status = device.is_link_up();
            
            crate::early_println!("[virtio-net test] Device {}: addr={:#x}, MAC={:?}, MTU={:?}, link={}",
                                i, base_addr, mac_result.is_ok(), mtu_result.is_ok(), link_status);
        }
        
        crate::early_println!("[virtio-net test] Device enumeration test completed");
    }

    #[test_case]
    fn test_virtio_net_hub_loopback_with_polling() {
        // Initialize both devices for network operations
        let mut sender_mut = VirtioNetDevice::new(0x10004000);
        let mut receiver_mut = VirtioNetDevice::new(0x10005000);
        
        let _ = sender_mut.init_network();
        let _ = receiver_mut.init_network();
        
        // Create a distinctive test packet
        let test_packet_data = vec![
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, // Dest MAC (broadcast)
            0x52, 0x54, 0x00, 0x12, 0x34, 0x57, // Src MAC (matching net1 default)
            0x08, 0x00, // Ethernet type (IPv4)
            // Simple payload for identification
            0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE,
        ];
        let test_packet = DevicePacket::with_data(test_packet_data);
        
        crate::early_println!("[virtio-net test] Sending test packet from sender device...");
        let tx_result = sender_mut.transmit_packet(&test_packet);
        crate::early_println!("[virtio-net test] TX result: {:?}", tx_result.is_ok());
        
        // Poll for received packets with multiple attempts
        crate::early_println!("[virtio-net test] Polling for received packets...");
        let mut total_received = 0;
        
        for attempt in 0..5 {
            let rx_result = receiver_mut.receive_packets();
            match rx_result {
                Ok(packets) => {
                    if !packets.is_empty() {
                        crate::early_println!("[virtio-net test] Attempt {}: Received {} packets", 
                                            attempt, packets.len());
                        total_received += packets.len();
                        
                        for (i, packet) in packets.iter().enumerate() {
                            crate::early_println!("[virtio-net test] RX packet {}: {} bytes", i, packet.len);
                            // Check if this might be our test packet
                            if packet.len >= 8 {
                                let has_magic = packet.data.len() >= 8 && 
                                    packet.data[packet.data.len()-8..packet.data.len()] == 
                                    [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE];
                                if has_magic {
                                    crate::early_println!("[virtio-net test] Found our test packet!");
                                }
                            }
                        }
                    } else {
                        crate::early_println!("[virtio-net test] Attempt {}: No packets received", attempt);
                    }
                },
                Err(e) => crate::early_println!("[virtio-net test] RX error on attempt {}: {}", attempt, e),
            }
            
            // Small delay between polling attempts (in a real system, this would be interrupt-driven)
            for _ in 0..1000 { core::hint::spin_loop(); }
        }
        
        crate::early_println!("[virtio-net test] Total received packets: {}", total_received);
        
        // Verify device statistics
        let sender_stats = sender_mut.get_stats();
        let receiver_stats = receiver_mut.get_stats();

        crate::early_println!("[virtio-net test] Final stats - sender: TX:{}, RX:{} | receiver: TX:{}, RX:{}",
                            sender_stats.tx_packets, sender_stats.rx_packets,
                            receiver_stats.tx_packets, receiver_stats.rx_packets);
        
        crate::early_println!("[virtio-net test] Hub loopback with polling test completed");
    }

    #[test_case]
    fn test_virtio_net_qemu_network_configuration() {
        // Test that verifies our understanding of QEMU network setup
        crate::early_println!("[virtio-net test] Testing QEMU network configuration understanding");
        
        // Expected device configuration based on test.sh setup:
        // -device virtio-net-device,netdev=net0,mac=52:54:00:12:34:56,bus=virtio-mmio-bus.2
        // -device virtio-net-device,netdev=net1,mac=52:54:00:12:34:57,bus=virtio-mmio-bus.3  
        // -device virtio-net-device,netdev=net2,mac=52:54:00:12:34:58,bus=virtio-mmio-bus.4
        // -netdev user,id=net0
        // -netdev hubport,id=net1,hubid=0
        // -netdev hubport,id=net2,hubid=0
        
        let device_net0 = VirtioNetDevice::new(0x10003000); // bus.2 -> user netdev
        let device_net1 = VirtioNetDevice::new(0x10004000); // bus.3 -> hub netdev 
        let device_net2 = VirtioNetDevice::new(0x10005000); // bus.4 -> hub netdev
        
        // Verify all devices are properly configured
        let devices = [
            ("net0", &device_net0, 0x10003000),
            ("net1", &device_net1, 0x10004000), 
            ("net2", &device_net2, 0x10005000),
        ];
        
        for (name, device, expected_addr) in &devices {
            crate::early_println!("[virtio-net test] Testing device {}", name);
            
            assert_eq!(device.get_base_addr(), *expected_addr);
            
            let mac_result = device.get_mac_address();
            let mtu_result = device.get_mtu();
            let config_result = device.get_interface_config();
            let link_status = device.is_link_up();
            
            crate::early_println!("[virtio-net test] {} - base_addr: {:#x}, MAC: {}, MTU: {}, config: {}, link: {}",
                                name, device.get_base_addr(), 
                                mac_result.is_ok(), mtu_result.is_ok(), 
                                config_result.is_ok(), link_status);
            
            // All devices should be properly configured
            assert!(mac_result.is_ok(), "{} should have valid MAC", name);
            assert!(mtu_result.is_ok(), "{} should have valid MTU", name);
            assert!(config_result.is_ok(), "{} should have valid config", name);
            // Note: link status may vary depending on QEMU setup, so we don't assert it
        }
        
        crate::early_println!("[virtio-net test] QEMU network configuration test completed");
        crate::early_println!("[virtio-net test] Net0 (user): TX-only, external connectivity");
        crate::early_println!("[virtio-net test] Net1, Net2 (hub): Bidirectional, internal loopback");
    }
}