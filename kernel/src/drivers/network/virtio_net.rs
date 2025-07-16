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
//! - Transmit queue (index 0): For sending packets to the network
//! - Receive queue (index 1): For receiving packets from the network
//!
//! Each network packet is handled through the VirtIO descriptor chain mechanism,
//! with proper memory management for packet buffers.

use alloc::{boxed::Box, vec::Vec, vec};
use spin::{Mutex, RwLock};

use core::mem;

use crate::defer;
use crate::device::{Device, DeviceType};
use crate::{
    device::network::{NetworkDevice, NetworkPacket, NetworkInterfaceConfig, MacAddress, NetworkStats}, 
    drivers::virtio::{device::{Register, VirtioDevice}, queue::{DescriptorFlag, VirtQueue}}
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
    virtqueues: Mutex<[VirtQueue<'static>; 2]>, // TX queue (0) and RX queue (1)
    config: RwLock<Option<NetworkInterfaceConfig>>,
    features: RwLock<u32>,
    stats: Mutex<NetworkStats>,
    initialized: Mutex<bool>,
    // Buffer management for received packets
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
            virtqueues: Mutex::new([VirtQueue::new(64), VirtQueue::new(64)]), // TX and RX queues
            config: RwLock::new(None),
            features: RwLock::new(0),
            stats: Mutex::new(NetworkStats::default()),
            initialized: Mutex::new(false),
            rx_buffers: Mutex::new(Vec::new()),
        };
        
        // Initialize virtqueues
        {
            let mut virtqueues = device.virtqueues.lock();
            for (i, queue) in virtqueues.iter_mut().enumerate() {
                queue.init();
            }
        }
        
        // Initialize the VirtIO device
        if device.init().is_err() {
            panic!("Failed to initialize VirtIO Network Device");
        }

        // Read device configuration
        device.read_device_config();

        device
    }
    
    /// Read device configuration from the VirtIO config space
    fn read_device_config(&mut self) {
        // Get actually negotiated features after VirtIO initialization
        let negotiated_features = self.read32_register(Register::DriverFeatures);
        *self.features.write() = negotiated_features;
        
        // Debug: Print negotiated features in test builds
        #[cfg(test)]
        {
            use crate::early_println;
            // Also read device features for debugging
            let device_features = self.read32_register(Register::DeviceFeatures);
            early_println!("[virtio-net] Device offers features: 0x{:x}", device_features);
            early_println!("[virtio-net] Negotiated features: 0x{:x}", negotiated_features);
            if negotiated_features & (1 << VIRTIO_NET_F_MRG_RXBUF) != 0 {
                early_println!("[virtio-net] MRG_RXBUF negotiated");
            } else {
                early_println!("[virtio-net] MRG_RXBUF NOT negotiated");
            }
            if negotiated_features & (1 << VIRTIO_NET_F_MAC) != 0 {
                early_println!("[virtio-net] MAC address supported");
            }
            if negotiated_features & (1 << VIRTIO_NET_F_STATUS) != 0 {
                early_println!("[virtio-net] Status supported");
            }
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
        let rx_queue = &mut virtqueues[1]; // RX queue is index 1
        
        // Pre-populate RX queue with buffers using separate descriptors for header and data
        let buffer_count = rx_queue.desc.len().min(8); // Use fewer buffers to avoid issues
        
        for _ in 0..buffer_count {
            let hdr_size = self.get_header_size();
            let packet_size = self.get_mtu().unwrap_or(DEFAULT_MTU) + 64;
            
            // Allocate separate header buffer
            let hdr_buffer = vec![0u8; hdr_size].into_boxed_slice();
            let hdr_ptr = Box::into_raw(hdr_buffer);
            
            // Allocate separate packet data buffer
            let data_buffer = vec![0u8; packet_size].into_boxed_slice();
            let data_ptr = Box::into_raw(data_buffer);
            
            // Allocate two descriptors for the RX chain
            let hdr_desc_idx = rx_queue.alloc_desc().ok_or("Failed to allocate RX header descriptor")?;
            let data_desc_idx = rx_queue.alloc_desc().ok_or("Failed to allocate RX data descriptor")?;
            
            // Setup header descriptor (first element - device writes virtio-net header here)
            rx_queue.desc[hdr_desc_idx].addr = hdr_ptr as *mut u8 as u64;
            rx_queue.desc[hdr_desc_idx].len = hdr_size as u32;
            rx_queue.desc[hdr_desc_idx].flags = (DescriptorFlag::Write as u16) | (DescriptorFlag::Next as u16);
            rx_queue.desc[hdr_desc_idx].next = data_desc_idx as u16;
            
            // Setup data descriptor (second element - device writes packet data here)
            rx_queue.desc[data_desc_idx].addr = data_ptr as *mut u8 as u64;
            rx_queue.desc[data_desc_idx].len = packet_size as u32;
            rx_queue.desc[data_desc_idx].flags = DescriptorFlag::Write as u16; // Device writes, no next
            rx_queue.desc[data_desc_idx].next = 0;
            
            // Add header descriptor to available ring (starts the chain)
            rx_queue.push(hdr_desc_idx)?;
            
            // Store buffer pointers for cleanup
            self.rx_buffers.lock().push(unsafe { Box::from_raw(hdr_ptr) });
            self.rx_buffers.lock().push(unsafe { Box::from_raw(data_ptr) });
        }
        
        // Notify device about available RX buffers
        self.notify(1); // Notify RX queue
        
        Ok(())
    }
    
    /// Process a single packet transmission
    fn transmit_packet(&self, packet: &NetworkPacket) -> Result<(), &'static str> {
        // Always use basic header since we don't support mergeable RX buffers
        let hdr_size = mem::size_of::<VirtioNetHdrBasic>();
        let total_size = hdr_size + packet.len;
        
        // Allocate single contiguous buffer with proper alignment
        let mut combined_buffer = vec![0u8; total_size];
        
        // Copy basic header to the beginning of buffer
        let net_hdr_basic = VirtioNetHdrBasic::new();
        unsafe {
            let hdr_ptr = &net_hdr_basic as *const VirtioNetHdrBasic as *const u8;
            core::ptr::copy_nonoverlapping(hdr_ptr, combined_buffer.as_mut_ptr(), hdr_size);
        }
        
        // Copy packet data after header
        combined_buffer[hdr_size..].copy_from_slice(&packet.data[..packet.len]);
        
        // Convert to boxed slice for stable memory address
        let buffer_box = combined_buffer.into_boxed_slice();
        let buffer_ptr = Box::into_raw(buffer_box);
        
        defer! {
            // Cleanup memory
            unsafe {
                drop(Box::from_raw(buffer_ptr));
            }
        }
        
        let mut virtqueues = self.virtqueues.lock();
        let tx_queue = &mut virtqueues[0]; // TX queue is index 0
        
        // Allocate single descriptor for the combined buffer
        let desc_idx = tx_queue.alloc_desc().ok_or("Failed to allocate TX descriptor")?;
        
        // Setup descriptor for the combined buffer (device readable)
        tx_queue.desc[desc_idx].addr = buffer_ptr as *mut u8 as u64;
        tx_queue.desc[desc_idx].len = total_size as u32;
        tx_queue.desc[desc_idx].flags = 0; // No flags, device reads from this buffer
        tx_queue.desc[desc_idx].next = 0; // No chaining
        
        // Submit the request to the queue
        tx_queue.push(desc_idx)?;
        
        // Notify the device
        self.notify(0); // Notify TX queue
        
        // Wait for transmission (polling)
        while tx_queue.is_busy() {}
        
        // Get completion
        let _completed_desc = tx_queue.pop().ok_or("No TX completion")?;
        
        // Update statistics
        {
            let mut stats = self.stats.lock();
            stats.tx_packets += 1;
            stats.tx_bytes += packet.len as u64;
        }
        
        Ok(())
    }
    
    /// Process received packets from RX queue
    fn process_received_packets(&self) -> Result<Vec<NetworkPacket>, &'static str> {
        let mut packets = Vec::new();
        let mut virtqueues = self.virtqueues.lock();
        let rx_queue = &mut virtqueues[1]; // RX queue is index 1
        
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
                    let packet = NetworkPacket::with_data(packet_data.to_vec());
                    packets.push(packet);
                }
            }
            
            // Recycle the buffer by putting it back in the RX queue
            rx_queue.desc[desc_idx].flags = DescriptorFlag::Write as u16;
            rx_queue.push(desc_idx)?;
        }
        
        // Notify device about recycled buffers
        if !packets.is_empty() {
            self.notify(1); // Notify RX queue
            
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

impl VirtioDevice for VirtioNetDevice {
    fn get_base_addr(&self) -> usize {
        self.base_addr
    }
    
    fn get_virtqueue_count(&self) -> usize {
        2 // TX and RX queues
    }
    
    fn get_supported_features(&self, device_features: u32) -> u32 {
        // Accept basic network features (don't include high-bit ring features for now)
        let supported_features = (1 << VIRTIO_NET_F_MAC) |          // bit 5
                                (1 << VIRTIO_NET_F_STATUS) |         // bit 16  
                                (1 << VIRTIO_NET_F_MTU);             // bit 3
        
        device_features & supported_features
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
    
    fn send_packet(&self, packet: NetworkPacket) -> Result<(), &'static str> {
        if !self.is_link_up() {
            return Err("Link is down");
        }
        
        self.transmit_packet(&packet)
    }
    
    fn receive_packets(&self) -> Result<Vec<NetworkPacket>, &'static str> {
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
    fn test_virtio_net_packet_operations() {
        let mut device = VirtioNetDevice::new(0x10003000);
        device.init_network().unwrap();
        
        // Test packet creation
        let test_data = vec![0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0xAA, 0xBB];
        let packet = NetworkPacket::with_data(test_data.clone());
        assert_eq!(packet.as_slice(), &test_data);
        
        // Test packet sending (will succeed in test environment)
        if device.is_link_up() {
            assert!(device.send_packet(packet).is_ok());
            
            // Check statistics
            let stats = device.get_stats();
            assert_eq!(stats.tx_packets, 1);
            assert_eq!(stats.tx_bytes, test_data.len() as u64);
        }
        
        // Test packet receiving
        let received = device.receive_packets().unwrap();
        // In test environment, no packets will be received
        assert_eq!(received.len(), 0);
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
                let packet = NetworkPacket::with_data(data);
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
}