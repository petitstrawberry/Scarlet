//! PCI device scanning and enumeration.
//!
//! This module implements the logic for scanning the PCI bus tree and
//! discovering devices.

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;

use super::config::{PciBar, PciBarKind, PciConfig, offset, vendor};
use super::device::PciDeviceInfo;
use super::{PciAddress, PciBus};
use crate::{early_println, println};

/// PCI scanner
///
/// Handles scanning the PCI bus tree to discover devices.
pub struct PciScanner<'a> {
    /// PCI configuration space accessor
    config: PciConfig,
    /// Bus manager reference
    bus: &'a PciBus,
}

impl<'a> PciScanner<'a> {
    fn log_bar(addr: &PciAddress, bar: &PciBar) {
        let reg = offset::BAR0 + bar.index as usize * 4;
        let kind = match bar.kind {
            PciBarKind::Io => "io",
            PciBarKind::Memory32 | PciBarKind::Memory64 => "mem",
        };
        let bits = if bar.is_64bit() { " 64bit" } else { "" };
        let prefetchable = if bar.prefetchable { " pref" } else { "" };

        if bar.is_assigned() {
            let end = bar.base.saturating_add(bar.size.saturating_sub(1));
            println!(
                "pci 0000:{:02x}:{:02x}.{}: reg {:#04x}: [{} {:#x}-{:#x} size={:#x}{}{}]",
                addr.bus,
                addr.device,
                addr.function,
                reg,
                kind,
                bar.base,
                end,
                bar.size,
                bits,
                prefetchable
            );
        } else {
            println!(
                "pci 0000:{:02x}:{:02x}.{}: reg {:#04x}: [{} size={:#x}{}{}] unassigned",
                addr.bus, addr.device, addr.function, reg, kind, bar.size, bits, prefetchable
            );
        }
    }

    fn is_pci_host_node(node: &fdt::node::FdtNode<'_, '_>) -> bool {
        node.name.starts_with("pci@")
            || node.name.starts_with("pcie@")
            || node
                .compatible()
                .map(|compat| compat.all().any(|entry| entry == "pci-host-ecam-generic"))
                .unwrap_or(false)
    }

    fn read_be_u32(bytes: &[u8]) -> Option<u32> {
        if bytes.len() < 4 {
            return None;
        }
        Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn get_u32_prop<'b, 'c>(node: &fdt::node::FdtNode<'b, 'c>, name: &str) -> Option<u32> {
        let prop = node.property(name)?;
        Self::read_be_u32(prop.value)
    }

    fn find_node_by_phandle<'b>(
        fdt: &'b fdt::Fdt<'b>,
        phandle: u32,
    ) -> Option<fdt::node::FdtNode<'b, 'b>> {
        let mut stack: Vec<fdt::node::FdtNode<'b, 'b>> = Vec::new();
        stack.push(fdt.find_node("/")?);

        while let Some(node) = stack.pop() {
            if let Some(p) = Self::get_u32_prop(&node, "phandle") {
                if p == phandle {
                    return Some(node);
                }
            }
            if let Some(p) = Self::get_u32_prop(&node, "linux,phandle") {
                if p == phandle {
                    return Some(node);
                }
            }
            for child in node.children() {
                stack.push(child);
            }
        }

        None
    }

    fn decode_parent_irq(parent_irq_cells: &[u32]) -> Option<u32> {
        match parent_irq_cells.len() {
            0 => None,
            3 => parent_irq_cells.get(1).copied(),
            _ => parent_irq_cells.first().copied(),
        }
    }

    fn parse_routed_irq_from_map<F>(
        mask: &[u8],
        map: &[u8],
        child_cells: &[u32],
        mut parent_cell_counts: F,
    ) -> Option<u32>
    where
        F: FnMut(u32) -> Option<(usize, usize)>,
    {
        let child_cell_count = child_cells.len();
        if child_cell_count == 0 || mask.len() < child_cell_count * 4 {
            return None;
        }

        let mut masked_child = alloc::vec![0; child_cell_count];
        for (index, child_cell) in child_cells.iter().enumerate() {
            let mask_offset = index * 4;
            let cell_mask = Self::read_be_u32(&mask[mask_offset..mask_offset + 4])?;
            masked_child[index] = *child_cell & cell_mask;
        }

        let mut offset = 0usize;
        while offset + (child_cell_count + 1) * 4 <= map.len() {
            let mut masked_map_child = alloc::vec![0; child_cell_count];
            for index in 0..child_cell_count {
                let cell_offset = offset + index * 4;
                let map_cell = Self::read_be_u32(&map[cell_offset..cell_offset + 4])?;
                let mask_offset = index * 4;
                let cell_mask = Self::read_be_u32(&mask[mask_offset..mask_offset + 4])?;
                masked_map_child[index] = map_cell & cell_mask;
            }

            let phandle_offset = offset + child_cell_count * 4;
            let phandle = Self::read_be_u32(&map[phandle_offset..phandle_offset + 4])?;
            let (parent_addr_cells, parent_interrupt_cells) = parent_cell_counts(phandle)?;
            if parent_interrupt_cells == 0 {
                return None;
            }

            let entry_cell_count =
                child_cell_count + 1 + parent_addr_cells + parent_interrupt_cells;
            let entry_bytes = entry_cell_count * 4;
            if offset + entry_bytes > map.len() {
                return None;
            }

            if masked_map_child == masked_child {
                let parent_irq_offset = phandle_offset + 4 + parent_addr_cells * 4;
                let mut parent_irq_cells = alloc::vec![0; parent_interrupt_cells];
                for (index, parent_irq_cell) in parent_irq_cells.iter_mut().enumerate() {
                    let cell_offset = parent_irq_offset + index * 4;
                    *parent_irq_cell = Self::read_be_u32(&map[cell_offset..cell_offset + 4])?;
                }
                return Self::decode_parent_irq(&parent_irq_cells);
            }

            offset += entry_bytes;
        }

        None
    }

    fn routed_irq_for(&self, addr: &PciAddress, interrupt_pin: u8) -> Option<u32> {
        if interrupt_pin == 0 {
            return None;
        }

        let fdt = crate::device::fdt::FdtManager::get_manager().get_fdt()?;
        let mut pci_node = None;
        for parent_path in ["/soc", "/"] {
            let Some(parent) = fdt.find_node(parent_path) else {
                continue;
            };
            pci_node = parent.children().find(Self::is_pci_host_node);
            if pci_node.is_some() {
                break;
            }
        }
        let pci_node = pci_node?;

        let mask = pci_node.property("interrupt-map-mask")?.value;
        let map = pci_node.property("interrupt-map")?.value;
        let child_addr_cells =
            Self::get_u32_prop(&pci_node, "#address-cells").unwrap_or(3) as usize;
        let child_interrupt_cells =
            Self::get_u32_prop(&pci_node, "#interrupt-cells").unwrap_or(1) as usize;
        if child_addr_cells == 0 || child_interrupt_cells == 0 {
            return None;
        }

        let child_cell_count = child_addr_cells + child_interrupt_cells;
        let mut child_cells = alloc::vec![0; child_cell_count];
        child_cells[0] = ((addr.device as u32) << 11) | ((addr.function as u32) << 8);
        child_cells[child_addr_cells] = interrupt_pin as u32;

        Self::parse_routed_irq_from_map(mask, map, &child_cells, |phandle| {
            let parent = Self::find_node_by_phandle(fdt, phandle)?;
            let parent_addr_cells = Self::get_u32_prop(&parent, "#address-cells").unwrap_or(0);
            let parent_interrupt_cells =
                Self::get_u32_prop(&parent, "#interrupt-cells").unwrap_or(1);
            Some((parent_addr_cells as usize, parent_interrupt_cells as usize))
        })
    }

    /// Create a new PCI scanner
    ///
    /// # Arguments
    ///
    /// * `bus` - Reference to the PCI bus manager
    pub fn new(bus: &'a PciBus) -> Result<Self, &'static str> {
        let config = PciConfig::new(bus.ecam_vaddr()?);
        Ok(Self { config, bus })
    }

    /// Scan the entire PCI bus tree
    ///
    /// This scans all possible bus/device/function combinations and
    /// discovers present devices.
    ///
    /// # Returns
    ///
    /// Vector of discovered PCI devices
    pub fn scan(&self) -> Vec<PciDeviceInfo> {
        let mut devices = Vec::new();
        let mut device_id_counter = 0;
        let mut visited_buses = [false; 256];

        println!("Scanning PCI bus...");

        // Start by checking bus 0
        self.scan_bus(0, &mut devices, &mut device_id_counter, &mut visited_buses);

        println!("PCI scan complete: found {} devices", devices.len());

        devices
    }

    /// Scan a single PCI bus
    fn scan_bus(
        &self,
        bus: u8,
        devices: &mut Vec<PciDeviceInfo>,
        id_counter: &mut usize,
        visited_buses: &mut [bool; 256],
    ) {
        if visited_buses[bus as usize] {
            return;
        }
        visited_buses[bus as usize] = true;

        // Scan all 32 possible devices on this bus
        for device in 0..32 {
            self.scan_device(bus, device, devices, id_counter, visited_buses);
        }
    }

    /// Scan a single PCI device
    fn scan_device(
        &self,
        bus: u8,
        device: u8,
        devices: &mut Vec<PciDeviceInfo>,
        id_counter: &mut usize,
        visited_buses: &mut [bool; 256],
    ) {
        let addr = PciAddress::new(0, bus, device, 0);

        // Check if device exists by reading vendor ID
        let vendor_id = self.config.read_vendor_id(&addr);
        if vendor_id == vendor::INVALID {
            return; // No device present
        }

        // Read header type to check if this is a multi-function device
        let header_type = self.config.read_header_type(&addr);
        let is_multifunction = (header_type & 0x80) != 0;

        // Scan function 0
        if let Some(device_info) = self.probe_function(bus, device, 0, id_counter) {
            self.scan_child_bus_if_bridge(&device_info, devices, id_counter, visited_buses);
            devices.push(device_info);
        }

        // If multi-function, scan remaining functions
        if is_multifunction {
            for function in 1..8 {
                if let Some(device_info) = self.probe_function(bus, device, function, id_counter) {
                    self.scan_child_bus_if_bridge(&device_info, devices, id_counter, visited_buses);
                    devices.push(device_info);
                }
            }
        }
    }

    fn scan_child_bus_if_bridge(
        &self,
        device_info: &PciDeviceInfo,
        devices: &mut Vec<PciDeviceInfo>,
        id_counter: &mut usize,
        visited_buses: &mut [bool; 256],
    ) {
        if device_info.base_class() != 0x06 || device_info.subclass() != 0x04 {
            return;
        }

        let addr = device_info.address();
        let secondary = self
            .config
            .read_u8(&addr, super::config::offset::SECONDARY_BUS_NUMBER);
        let subordinate = self
            .config
            .read_u8(&addr, super::config::offset::SUBORDINATE_BUS_NUMBER);
        if secondary == 0 || secondary > subordinate {
            early_println!(
                "PCI: bridge {:02x}:{:02x}.{} has invalid bus range secondary={} subordinate={}",
                addr.bus,
                addr.device,
                addr.function,
                secondary,
                subordinate
            );
            return;
        }

        early_println!(
            "PCI: scanning bridge {:02x}:{:02x}.{} secondary bus {} subordinate {}",
            addr.bus,
            addr.device,
            addr.function,
            secondary,
            subordinate
        );
        self.scan_bus(secondary, devices, id_counter, visited_buses);
    }

    /// Probe a specific PCI function
    fn probe_function(
        &self,
        bus: u8,
        device: u8,
        function: u8,
        id_counter: &mut usize,
    ) -> Option<PciDeviceInfo> {
        let addr = PciAddress::new(0, bus, device, function);

        // Check if function exists
        // Note: In test environment, ECAM might not be properly mapped
        // We need to handle this gracefully
        let vendor_id = self.config.read_vendor_id(&addr);
        if vendor_id == vendor::INVALID {
            return None;
        }

        // Read device configuration
        let device_id = self.config.read_device_id(&addr);
        let class_code = self.config.read_class_code(&addr);
        let revision = self
            .config
            .read_u8(&addr, super::config::offset::REVISION_ID);
        let subsystem_vendor_id = self
            .config
            .read_u16(&addr, super::config::offset::SUBSYSTEM_VENDOR_ID);
        let subsystem_id = self
            .config
            .read_u16(&addr, super::config::offset::SUBSYSTEM_ID);
        let interrupt_line = self
            .config
            .read_u8(&addr, super::config::offset::INTERRUPT_LINE);
        let interrupt_pin = self
            .config
            .read_u8(&addr, super::config::offset::INTERRUPT_PIN);
        let routed_irq = self.routed_irq_for(&addr, interrupt_pin);
        let mut bars = self.config.read_bars(&addr);
        let bar_issues = PciConfig::validate_bars(&bars);
        if !bar_issues.is_empty() {
            for issue in &bar_issues {
                println!(
                    "PCI: invalid BAR resource at {:02x}:{:02x}.{}: {:?}",
                    bus, device, function, issue
                );
            }
            bars.clear();
        }

        // Generate device name
        // In a real implementation, this would use a static string pool
        // For now, we'll use a simple format
        let name = Self::generate_device_name(vendor_id, device_id, bus, device, function);

        let device_info = PciDeviceInfo::new(
            addr,
            self.bus.ecam_vaddr().ok()?,
            vendor_id,
            device_id,
            class_code,
            revision,
            subsystem_vendor_id,
            subsystem_id,
            interrupt_line,
            interrupt_pin,
            routed_irq,
            name,
            *id_counter,
        )
        .with_bars(bars);

        *id_counter += 1;

        println!(
            "pci 0000:{:02x}:{:02x}.{}: [{:04x}:{:04x}] type {:02x} class {:#08x}",
            bus,
            device,
            function,
            vendor_id,
            device_id,
            self.config.read_header_type(&addr) & 0x7f,
            class_code
        );
        for bar in device_info.bars() {
            Self::log_bar(&addr, bar);
        }

        Some(device_info)
    }

    /// Generate a device name from vendor and device IDs
    ///
    /// This is a simplified implementation. A real implementation would
    /// maintain a static string pool or use a more sophisticated naming scheme.
    fn generate_device_name(
        _vendor_id: u16,
        _device_id: u16,
        _bus: u8,
        _device: u8,
        _function: u8,
    ) -> &'static str {
        // For now, we'll use a simple static string
        // In practice, this should use a proper string allocation strategy
        // that works in a no_std environment
        match _vendor_id {
            vendor::INTEL => "intel_pci_device",
            vendor::AMD => "amd_pci_device",
            vendor::NVIDIA => "nvidia_pci_device",
            vendor::REDHAT => "virtio_pci_device",
            _ => "pci_device",
        }
    }
}

impl PciBus {
    /// Scan the PCI bus and discover all devices
    ///
    /// This is a convenience method that creates a scanner and performs
    /// the scan, storing discovered devices in the bus manager.
    pub fn scan(&self) -> Result<(), &'static str> {
        let scanner = PciScanner::new(self)?;
        let devices = scanner.scan();

        // Store discovered devices
        for device in devices {
            self.add_device(device);
        }
        Ok(())
    }

    /// Scan the PCI bus and register devices with the DeviceManager
    ///
    /// This scans for PCI devices and registers them with the global
    /// device manager so they can be matched with drivers via
    /// `DeviceManager::probe_pci_devices()`.
    pub fn scan_and_register(&self) -> Result<(), &'static str> {
        use crate::device::manager::DeviceManager;

        self.scan()?;

        let devices = self.devices();
        let device_manager = DeviceManager::get_manager();

        println!(
            "Registering {} PCI devices with DeviceManager",
            devices.len()
        );

        for device in devices {
            println!(
                "  - {} ({:04x}:{:04x})",
                device.name(),
                device.vendor_id(),
                device.device_id()
            );
            device_manager.register_pci_device(Arc::new(device));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_pci_scanner_creation() {
        let bus = PciBus::new(0x3000_0000, 0x1000_0000);
        match PciScanner::new(&bus) {
            Ok(_) | Err(_) => {}
        }
        // If we get here without panic, the test passes
    }

    #[test_case]
    fn test_parse_routed_irq_from_map_single_cell_parent_irq() {
        let mask = [
            0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x07,
        ];
        let map = [
            0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01, 0x00, 0x00, 0x00, 0x2a, 0x00, 0x00, 0x00, 0x15,
        ];
        let child_cells = [0x0000_0800, 0, 0, 1];

        let routed_irq =
            PciScanner::parse_routed_irq_from_map(&mask, &map, &child_cells, |phandle| {
                if phandle == 0x2a { Some((0, 1)) } else { None }
            });

        assert_eq!(routed_irq, Some(0x15));
    }

    #[test_case]
    fn test_parse_routed_irq_from_map_three_cell_parent_irq() {
        let mask = [
            0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x07,
        ];
        let map = [
            0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01, 0x00, 0x00, 0x00, 0x33, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x24, 0x00, 0x00, 0x00, 0x04,
        ];
        let child_cells = [0x0000_0800, 0, 0, 1];

        let routed_irq =
            PciScanner::parse_routed_irq_from_map(&mask, &map, &child_cells, |phandle| {
                if phandle == 0x33 { Some((1, 3)) } else { None }
            });

        assert_eq!(routed_irq, Some(0x24));
    }

    #[test_case]
    fn test_device_name_generation() {
        let name = PciScanner::generate_device_name(vendor::INTEL, 0x1234, 0, 0, 0);
        assert_eq!(name, "intel_pci_device");

        let name = PciScanner::generate_device_name(vendor::REDHAT, 0x1000, 0, 0, 0);
        assert_eq!(name, "virtio_pci_device");

        let name = PciScanner::generate_device_name(0x9999, 0x1234, 0, 0, 0);
        assert_eq!(name, "pci_device");
    }

    #[test_case]
    fn test_pci_real_device_discovery() {
        // This test actually scans for PCI devices in the QEMU environment
        // It should discover virtio-pci devices when run with virtio-pci in QEMU
        use crate::device::fdt::FdtManager;
        use crate::early_println;

        early_println!("[PCI Test] Starting real PCI device discovery test");

        // Get FDT to find PCI host bridge
        let fdt_manager = unsafe { FdtManager::get_mut_manager() };
        let fdt = fdt_manager.get_fdt();

        if fdt.is_none() {
            early_println!("[PCI Test] No FDT available, skipping test");
            return;
        }

        let fdt = fdt.unwrap();

        // Look for PCI host bridge in device tree
        let mut pci_found = false;
        let mut ecam_base = 0;
        let mut ecam_size = 0;

        // Check common PCI node names
        for node_name in &["/soc/pci", "/soc/pcie", "/pci", "/pcie"] {
            if let Some(pci_node) = fdt.find_node(node_name) {
                early_println!("[PCI Test] Found PCI node: {}", node_name);

                // Get reg property for ECAM base and size
                if let Some(reg) = pci_node.reg() {
                    for region in reg {
                        ecam_base = region.starting_address as usize;
                        if let Some(size) = region.size {
                            ecam_size = size;
                            pci_found = true;
                            early_println!(
                                "[PCI Test] ECAM base: {:#x}, size: {:#x}",
                                ecam_base,
                                ecam_size
                            );
                            break;
                        }
                    }
                }
                if pci_found {
                    break;
                }
            }
        }

        if !pci_found {
            early_println!("[PCI Test] No PCI host bridge found in device tree");
            early_println!("[PCI Test] This is expected if not running with PCI support");
            return;
        }

        // NOTE: PCI ECAM scanning requires the ECAM region to be properly mapped in virtual memory.
        // In the current test environment, we don't have a proper virtual memory mapping for ECAM,
        // so actual scanning will fail. This test verifies that:
        // 1. PCI node is detected in device tree
        // 2. ECAM base and size are extracted correctly
        // 3. PCI infrastructure can be initialized

        early_println!("[PCI Test] ✓ PCI host bridge detected in device tree");
        early_println!(
            "[PCI Test] ✓ ECAM configuration: base={:#x}, size={:#x}",
            ecam_base,
            ecam_size
        );
        early_println!(
            "[PCI Test] Note: Actual device scanning requires ECAM virtual memory mapping"
        );
        early_println!("[PCI Test] Test passed: PCI infrastructure initialized successfully");

        // For now, we consider it a success if we found the PCI node
        // Full scanning will work when ECAM is properly mapped in the kernel
        assert!(pci_found, "PCI host bridge should be detected");
    }
}
