#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::arch::mmio;
use scarlet::device::manager::DeviceManager;
use scarlet::interrupt::msi::{
    MsiAllocation, MsiController, MsiError, MsiMessage, MsiRequest, MsiRequestFlags, MsiVector,
};
use scarlet::sync::Mutex;

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

/// Per-port Apple Silicon MSI configuration and allocation state.
pub struct MsiPortConfig {
    port_base: usize,
    base_vector: u32,
    num_vectors: u32,
    allocated: Mutex<alloc::vec::Vec<bool>>,
}

impl MsiPortConfig {
    /// Create a new Apple MSI port configuration.
    ///
    /// # Arguments
    ///
    /// * `port_base` - MMIO base address for the PCIe port registers.
    /// * `base_vector` - First Apple MSI vector owned by this port.
    /// * `num_vectors` - Number of vectors exposed by this port.
    ///
    /// # Returns
    ///
    /// New MSI port configuration with all vectors initially free.
    pub fn new(port_base: usize, base_vector: u32, num_vectors: u32) -> Self {
        let n = num_vectors as usize;
        Self {
            port_base,
            base_vector,
            num_vectors,
            allocated: Mutex::new(alloc::vec![false; n]),
        }
    }

    /// Allocate one vector from this MSI port.
    ///
    /// # Returns
    ///
    /// The allocated Apple MSI vector number, or `None` when the port is full.
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

    /// Free one vector previously allocated from this MSI port.
    ///
    /// Invalid vector numbers outside this port's range are ignored.
    ///
    /// # Arguments
    ///
    /// * `vector` - Apple MSI vector number to release.
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

    /// Enable MSI delivery for this PCIe port in hardware.
    ///
    /// Programs the port MSI enable bit, fixed Apple Silicon doorbell address,
    /// and the port's base vector into the PCIe port MMIO registers.
    pub fn enable_msi(&self) {
        // SAFETY: port_base is within the MMIO-mapped PCIe port region
        unsafe {
            mmio::write32(self.port_base + PORT_MSICFG_OFFSET, PORT_MSICFG_ENABLE);
            mmio::write32(self.port_base + PORT_MSIADDR_OFFSET, MSI_DOORBELL_ADDR);
            mmio::write32(self.port_base + PORT_MSIBASE_OFFSET, self.base_vector);
        }
    }

    /// Return the fixed Apple Silicon MSI doorbell address.
    ///
    /// # Returns
    ///
    /// Physical MSI doorbell address used in MSI messages.
    pub fn doorbell_addr(&self) -> u32 {
        MSI_DOORBELL_ADDR
    }

    /// Return the first vector owned by this MSI port.
    ///
    /// # Returns
    ///
    /// Base Apple MSI vector number.
    pub fn base_vector(&self) -> u32 {
        self.base_vector
    }

    /// Return the number of vectors owned by this MSI port.
    ///
    /// # Returns
    ///
    /// Count of Apple MSI vectors exposed by this port.
    pub fn num_vectors(&self) -> u32 {
        self.num_vectors
    }
}

/// `MsiController` adapter for one Apple Silicon MSI port.
pub struct AppleMsiController {
    port: Arc<MsiPortConfig>,
    phandle: u32,
}

impl AppleMsiController {
    /// Create an Apple MSI controller adapter.
    ///
    /// # Arguments
    ///
    /// * `port` - Low-level Apple MSI port configuration backing allocations.
    /// * `phandle` - Firmware phandle used to register this MSI controller.
    ///
    /// # Returns
    ///
    /// New Apple MSI controller adapter.
    pub fn new(port: Arc<MsiPortConfig>, phandle: u32) -> Self {
        Self { port, phandle }
    }

    /// Return the firmware phandle associated with this controller.
    ///
    /// # Returns
    ///
    /// Firmware phandle used for device-manager MSI controller lookup.
    pub fn phandle(&self) -> u32 {
        self.phandle
    }
}

impl MsiController for AppleMsiController {
    fn name(&self) -> &'static str {
        "apple-msi"
    }

    fn allocate_vectors(&self, request: MsiRequest) -> Result<MsiAllocation, MsiError> {
        if request.count == 0 {
            return Err(MsiError::InvalidRequest);
        }

        if request.flags.contains(MsiRequestFlags::CONTIGUOUS) {
            // TODO: Classic MSI requests need true contiguous allocation by scanning for
            // `request.count` adjacent free entries. The current port helper allocates the
            // first free vector, so this adapter temporarily allocates one-by-one.
        }

        let mut vectors: Vec<MsiVector> = Vec::new();
        for _ in 0..request.count {
            let vector_number = match self.port.allocate_vector() {
                Some(vector_number) => vector_number,
                None => {
                    for vector in &vectors {
                        self.port.free_vector(vector.hwirq);
                    }
                    return Err(MsiError::NoVectors);
                }
            };

            vectors.push(MsiVector {
                // TODO: Allocate a real kernel Virq through InterruptManager once the
                // Apple AIC parent controller exposes MSI-domain integration.
                virq: vector_number,
                hwirq: vector_number,
                message: MsiMessage {
                    address: self.port.doorbell_addr() as u64,
                    data: vector_number,
                },
            });
        }

        Ok(MsiAllocation { vectors })
    }

    fn free_vectors(&self, allocation: &MsiAllocation) {
        for vector in &allocation.vectors {
            // `hwirq` is the controller-local Apple MSI vector number allocated above.
            self.port.free_vector(vector.hwirq);
        }
    }

    fn mask_vector(&self, vector: &MsiVector) -> Result<(), MsiError> {
        let _ = vector;
        // TODO: Implement Apple MSI vector masking through the PCIe port MMIO registers.
        Ok(())
    }

    fn unmask_vector(&self, vector: &MsiVector) -> Result<(), MsiError> {
        let _ = vector;
        // TODO: Implement Apple MSI vector unmasking through the PCIe port MMIO registers.
        Ok(())
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

/// Register a low-level Apple MSI port configuration.
///
/// # Arguments
///
/// * `config` - MSI port configuration to store in the local registry.
///
/// # Returns
///
/// Numeric port ID usable with [`get_msi_port`].
pub fn register_msi_port(config: MsiPortConfig) -> u32 {
    let mut guard = MSI_PORTS.lock();
    let id = guard.len() as u32;
    guard.push(Arc::new(config));
    id
}

/// Look up a low-level Apple MSI port configuration by ID.
///
/// # Arguments
///
/// * `id` - Port ID returned by [`register_msi_port`].
///
/// # Returns
///
/// Shared MSI port configuration, or `None` when the ID is unknown.
pub fn get_msi_port(id: u32) -> Option<Arc<MsiPortConfig>> {
    let guard = MSI_PORTS.lock();
    guard.get(id as usize).cloned()
}

/// Register an Apple MSI controller with the kernel device manager.
///
/// # Arguments
///
/// * `port` - Low-level Apple MSI port configuration backing the controller.
/// * `phandle` - Firmware phandle identifying this MSI controller.
pub fn register_apple_msi_controller(port: Arc<MsiPortConfig>, phandle: u32) {
    let controller = AppleMsiController::new(port, phandle);
    DeviceManager::get_manager().register_msi_controller(phandle, Arc::new(controller));
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_allocate_free_round_trip() {
        let port = Arc::new(MsiPortConfig::new(0, 32, 2));
        let controller = AppleMsiController::new(port, 0x100);

        let allocation = controller
            .allocate_vectors(MsiRequest {
                count: 2,
                target_cpu: 0,
                requester: None,
                flags: MsiRequestFlags::NONE,
            })
            .expect("Apple MSI vectors should allocate");

        assert_eq!(allocation.vectors.len(), 2);
        assert_eq!(allocation.vectors[0].hwirq, 32);
        assert_eq!(allocation.vectors[0].virq, 32);
        assert_eq!(
            allocation.vectors[0].message.address,
            MSI_DOORBELL_ADDR as u64
        );
        assert_eq!(allocation.vectors[0].message.data, 32);
        assert_eq!(allocation.vectors[1].hwirq, 33);

        assert!(
            controller
                .allocate_vectors(MsiRequest {
                    count: 1,
                    target_cpu: 0,
                    requester: None,
                    flags: MsiRequestFlags::NONE,
                })
                .is_err()
        );

        controller.free_vectors(&allocation);

        let allocation = controller
            .allocate_vectors(MsiRequest {
                count: 1,
                target_cpu: 0,
                requester: None,
                flags: MsiRequestFlags::NONE,
            })
            .expect("freed Apple MSI vector should allocate again");

        assert_eq!(allocation.vectors.len(), 1);
        assert_eq!(allocation.vectors[0].hwirq, 32);
    }
}

#[used]
static SCARLET_DRIVER_APPLE_MSI_ANCHOR: fn() = force_link;

/// Keep the Apple MSI driver crate linked when referenced by aggregate builds.
#[inline(never)]
pub fn force_link() {}
