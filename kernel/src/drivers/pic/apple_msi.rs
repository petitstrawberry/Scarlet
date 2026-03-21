//! Apple MSI (Message Signaled Interrupts) controller
//!
//! Manages MSI vector allocation for Apple Silicon PCIe ports.
//! Reference: asahi-linux `drivers/irqchip/irq-apple-aic.c` (MSI portion)
//!
//! # Architecture
//!
//! Apple MSI is not a separate interrupt controller. Instead:
//! - Each PCIe port has MSI config registers (PORT_MSICFG, PORT_MSIBASE, PORT_MSIADDR)
//! - MSI writes go to a doorbell address (0xfffff000) with the vector number as data
//! - The write is intercepted by the PCIe port and routed to AIC as a hardware IRQ
//! - The `msi-ranges` DT property defines the AIC IRQ range for MSI vectors
//!
//! This module provides MSI vector allocation from a shared bitmap.

#![allow(dead_code)]

use crate::arch::mmio;
use alloc::sync::Arc;
use spin::Mutex;

// =============================================================================
// MSI Configuration
// =============================================================================

/// MSI doorbell address — fixed for Apple Silicon
const MSI_DOORBELL_ADDR: u32 = 0xfffff000;

/// Maximum MSI vectors per port
const MSI_VECTORS_PER_PORT: usize = 32;

// =============================================================================
// MSI Port Configuration
// =============================================================================

pub struct MsiPortConfig {
    port_base: usize,
    base_vector: u32,
    num_vectors: u32,
    allocated: Mutex<alloc::vec::Vec<bool>>,
}

impl MsiPortConfig {
    pub fn new(port_base: usize, base_vector: u32, num_vectors: u32) -> Self {
        let n = num_vectors as usize;
        Self {
            port_base,
            base_vector,
            num_vectors,
            allocated: Mutex::new(alloc::vec![false; n]),
        }
    }

    pub fn allocate_vector(&self) -> Option<u32> {
        let mut alloc = self.allocated.lock();
        for (i, slot) in alloc.iter_mut().enumerate() {
            if !*slot {
                *slot = true;
                return Some(self.base_vector + i as u32);
            }
        }
        None
    }

    pub fn free_vector(&self, vector: u32) {
        if vector < self.base_vector || vector >= self.base_vector + self.num_vectors {
            return;
        }
        let idx = (vector - self.base_vector) as usize;
        let mut alloc = self.allocated.lock();
        if idx < alloc.len() {
            alloc[idx] = false;
        }
    }

    pub fn enable_msi(&self) {
        // SAFETY: port_base is within the MMIO-mapped PCIe port region
        unsafe {
            mmio::write32(self.port_base + PORT_MSICFG_OFFSET, PORT_MSICFG_ENABLE);
            mmio::write32(self.port_base + PORT_MSIADDR_OFFSET, MSI_DOORBELL_ADDR);
            mmio::write32(self.port_base + PORT_MSIBASE_OFFSET, self.base_vector);
        }
    }

    pub fn doorbell_addr(&self) -> u32 {
        MSI_DOORBELL_ADDR
    }

    pub fn base_vector(&self) -> u32 {
        self.base_vector
    }

    pub fn num_vectors(&self) -> u32 {
        self.num_vectors
    }
}

// =============================================================================
// Register Offsets (within PCIe port MMIO)
// =============================================================================

const PORT_MSICFG_OFFSET: usize = 0x0124;
const PORT_MSIBASE_OFFSET: usize = 0x0128;
const PORT_MSIADDR_OFFSET: usize = 0x0168;

const PORT_MSICFG_ENABLE: u32 = 1 << 0;

// =============================================================================
// Global MSI Registry
// =============================================================================

static MSI_PORTS: Mutex<alloc::vec::Vec<Arc<MsiPortConfig>>> = Mutex::new(alloc::vec::Vec::new());

pub fn register_msi_port(config: MsiPortConfig) -> u32 {
    let mut guard = MSI_PORTS.lock();
    let id = guard.len() as u32;
    guard.push(Arc::new(config));
    id
}

pub fn get_msi_port(id: u32) -> Option<Arc<MsiPortConfig>> {
    let guard = MSI_PORTS.lock();
    guard.get(id as usize).cloned()
}
