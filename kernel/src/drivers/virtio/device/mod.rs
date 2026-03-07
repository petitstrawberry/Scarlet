//! Virtio device driver interface module.
//!

use core::{
    result::Result,
    sync::atomic::{AtomicUsize, Ordering},
};

use alloc::{boxed::Box, format, string::ToString, sync::Arc, vec};

use crate::{
    arch::io_mb,
    device::{
        manager::{DeviceManager, DriverPriority},
        platform::{
            resource::PlatformDeviceResourceType, PlatformDeviceDriver, PlatformDeviceInfo,
        },
        Device,
    },
    driver_initcall,
    drivers::{
        block::virtio_blk::VirtioBlockDevice, graphics::virtio_gpu::VirtioGpuDevice,
        network::virtio_net::VirtioNetDevice, virtio_input::VirtioInputDevice,
        virtio_rng::VirtioRngDevice,
    },
    early_println,
};

// Static counters for device naming
static BLOCK_COUNTER: AtomicUsize = AtomicUsize::new(0);
static NET_COUNTER: AtomicUsize = AtomicUsize::new(0);
static GPU_COUNTER: AtomicUsize = AtomicUsize::new(0);
static INPUT_COUNTER: AtomicUsize = AtomicUsize::new(0);
static RNG_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Register enum for Virtio devices
///
/// This enum represents the registers of the Virtio device.
/// Each variant corresponds to a specific register offset.
/// The offsets are defined in the Virtio specification.
/// The register offsets are used to access the device's configuration and status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Register {
    MagicValue = 0x00,
    Version = 0x04,
    DeviceId = 0x08,
    VendorId = 0x0c,
    DeviceFeatures = 0x10,
    DeviceFeaturesSel = 0x14,
    DriverFeatures = 0x20,
    DriverFeaturesSel = 0x24,
    GuestPageSize = 0x28,
    QueueSel = 0x30,
    QueueNumMax = 0x34,
    QueueNum = 0x38,
    QueueAlign = 0x3c,
    QueuePfn = 0x40,
    QueueReady = 0x44,
    QueueNotify = 0x50,
    InterruptStatus = 0x60,
    InterruptAck = 0x64,
    Status = 0x70,
    QueueDescLow = 0x80,
    QueueDescHigh = 0x84,
    DriverDescLow = 0x90,
    DriverDescHigh = 0x94,
    DeviceDescLow = 0xa0,
    DeviceDescHigh = 0xa4,
    DeviceConfig = 0x100,
}

impl Register {
    pub fn offset(&self) -> usize {
        *self as usize
    }

    pub fn from_offset(offset: usize) -> Self {
        match offset {
            0x00 => Register::MagicValue,
            0x04 => Register::Version,
            0x08 => Register::DeviceId,
            0x0c => Register::VendorId,
            0x10 => Register::DeviceFeatures,
            0x14 => Register::DeviceFeaturesSel,
            0x20 => Register::DriverFeatures,
            0x24 => Register::DriverFeaturesSel,
            0x28 => Register::GuestPageSize,
            0x30 => Register::QueueSel,
            0x34 => Register::QueueNumMax,
            0x38 => Register::QueueNum,
            0x3c => Register::QueueAlign,
            0x40 => Register::QueuePfn,
            0x44 => Register::QueueReady,
            0x50 => Register::QueueNotify,
            0x60 => Register::InterruptStatus,
            0x64 => Register::InterruptAck,
            0x70 => Register::Status,
            0x80 => Register::QueueDescLow,
            0x84 => Register::QueueDescHigh,
            0x90 => Register::DriverDescLow,
            0x94 => Register::DriverDescHigh,
            0xa0 => Register::DeviceDescLow,
            0xa4 => Register::DeviceDescHigh,
            _ => panic!("Invalid register offset"),
        }
    }
}

/// DeviceStatus enum for Virtio devices
///
/// This enum represents the status of the Virtio device.
/// Each variant corresponds to a specific status bit.
/// The status bits are defined in the Virtio specification.
#[derive(Debug, Clone, Copy)]
pub enum DeviceStatus {
    Reset = 0x00,
    Acknowledge = 0x01,
    Driver = 0x02,
    DriverOK = 0x04,
    FeaturesOK = 0x08,
    DeviceNeedReset = 0x40,
    Failed = 0x80,
}

impl DeviceStatus {
    /// Check if the status is set
    ///
    /// This method checks if the specified status bit is set in the given status.
    ///
    /// # Arguments
    ///
    /// * `status` - The status to check.
    ///
    /// # Returns
    ///
    /// Returns true if the status bit is set, false otherwise.
    pub fn is_set(&self, status: u32) -> bool {
        (status & *self as u32) != 0
    }

    /// Set the status bit
    ///
    /// This method sets the specified status bit in the given status.
    ///
    /// # Arguments
    ///
    /// * `status` - A mutable reference to the status to modify.
    ///
    pub fn set(&self, status: &mut u32) {
        *status |= *self as u32;
    }

    /// Clear the status bit
    ///
    /// This method clears the specified status bit in the given status.
    ///
    /// # Arguments
    ///
    /// * `status` - A mutable reference to the status to modify.
    ///
    pub fn clear(&self, status: &mut u32) {
        *status &= !(*self as u32);
    }

    /// Toggle the status bit
    ///
    /// This method toggles the specified status bit in the given status.
    ///
    /// # Arguments
    ///
    /// * `status` - A mutable reference to the status to modify.
    ///
    pub fn toggle(&self, status: &mut u32) {
        *status ^= *self as u32;
    }

    /// Convert from u32 to DeviceStatus
    ///
    /// This method converts a u32 value to the corresponding DeviceStatus variant.
    ///
    /// # Arguments
    ///
    /// * `status` - The u32 value to convert.
    ///
    /// # Returns
    ///
    /// Returns the corresponding DeviceStatus variant.
    ///
    pub fn from_u32(status: u32) -> Self {
        match status {
            0x00 => DeviceStatus::Reset,
            0x01 => DeviceStatus::Acknowledge,
            0x02 => DeviceStatus::Driver,
            0x04 => DeviceStatus::DriverOK,
            0x08 => DeviceStatus::FeaturesOK,
            0x40 => DeviceStatus::DeviceNeedReset,
            0x80 => DeviceStatus::Failed,
            _ => panic!("Invalid device status"),
        }
    }

    /// Convert DeviceStatus to u32
    ///
    /// This method converts the DeviceStatus variant to its corresponding u32 value.
    ///
    /// # Returns
    ///
    /// Returns the u32 value corresponding to the DeviceStatus variant.
    ///
    pub fn to_u32(&self) -> u32 {
        *self as u32
    }
}

/// VirtioDevice trait
///
/// This trait defines the interface for VirtIO devices.
/// It provides methods for initializing the device, accessing registers,
/// and performing device operations according to the VirtIO specification.
pub trait VirtioDevice {
    #[cfg(not(debug_assertions))]
    #[inline(never)]
    fn write32_register_slowpath(&self, addr: usize, value: u32) {
        io_mb();
        unsafe { crate::arch::mmio::write32(addr, value) };
        io_mb();
        // Some environments effectively require a read to flush a posted MMIO write.
        // Do NOT read back the just-written register: many virtio-mmio registers are write-only
        // (QueueNotify, queue address regs, DriverFeatures, etc.), and QEMU will warn.
        // Reading STATUS is safe and provides a single well-defined flush point.
        let status_addr = self.get_base_addr() + Register::Status.offset();
        unsafe {
            crate::arch::mmio::read32(status_addr);
        }
        io_mb();
    }

    fn debug_dump_mmio_state(&self, tag: &'static str) {
        #[cfg(debug_assertions)]
        {
            let base = self.get_base_addr();
            let magic = self.read32_register(Register::MagicValue);
            let version = self.read32_register(Register::Version);
            let device_id = self.read32_register(Register::DeviceId);
            let vendor_id = self.read32_register(Register::VendorId);
            let status = self.read32_register(Register::Status);
            let isr = self.read32_register(Register::InterruptStatus);

            crate::early_println!(
                "[virtio][{}] base={:#x} magic=0x{:08x} ver={} dev_id={} vendor=0x{:08x} status=0x{:02x} isr=0x{:02x}",
                tag,
                base,
                magic,
                version,
                device_id,
                vendor_id,
                status,
                isr
            );
        }

        #[cfg(not(debug_assertions))]
        {
            let _ = tag;
        }
    }

    fn debug_log_status_transition(&self, tag: &'static str, old: u32, new: u32, readback: u32) {
        #[cfg(debug_assertions)]
        {
            crate::early_println!(
                "[virtio][{}] status: old=0x{:02x} -> write=0x{:02x} -> readback=0x{:02x}",
                tag,
                old,
                new,
                readback
            );
        }

        #[cfg(not(debug_assertions))]
        {
            let _ = (tag, old, new, readback);
        }
    }

    fn wait_for_status_zero(
        &self,
        tag: &'static str,
        max_iters: usize,
    ) -> Result<(), &'static str> {
        // Virtio spec: after writing 0 (reset), driver should consider reset complete
        // when it reads back status == 0.
        for _ in 0..max_iters {
            let status = self.read32_register(Register::Status) & 0xff;
            if status == 0 {
                return Ok(());
            }
        }
        let final_status = self.read32_register(Register::Status) & 0xff;
        crate::early_println!(
            "[virtio][{}] reset wait timeout: status=0x{:02x}",
            tag,
            final_status
        );
        Err("Virtio device reset did not complete")
    }

    /// Initialize the device
    ///
    /// This method performs the standard VirtIO initialization sequence:
    /// 1. Reset the device
    /// 2. Acknowledge the device
    /// 3. Set driver status
    /// 4. Negotiate features
    /// 5. Set up virtqueues
    /// 6. Set driver OK status
    ///
    /// # Returns
    ///
    /// Returns Ok(negotiated_features) if initialization was successful,
    /// Err message otherwise
    fn init(&mut self) -> Result<u32, &'static str> {
        self.debug_dump_mmio_state("init:entry");

        // Verify device (Magic Value should be "virt")
        if self.read32_register(Register::MagicValue) != 0x74726976 {
            self.set_failed();
            return Err("Invalid Magic Value");
        }

        // Check device version
        let version = self.read32_register(Register::Version);
        if version != 2 {
            self.set_failed();
            return Err("Invalid Version");
        }

        // Reset device
        if let Err(e) = self.reset() {
            self.set_failed();
            return Err(e);
        }
        // self.debug_dump_mmio_state("init:after_reset");

        // Acknowledge device
        self.acknowledge();
        self.debug_dump_mmio_state("init:after_ack");

        // Set driver status
        self.driver();
        self.debug_dump_mmio_state("init:after_driver");

        // Negotiate features
        let negotiated_features = match self.negotiate_features() {
            Ok(features) => features,
            Err(e) => {
                self.set_failed();
                return Err(e);
            }
        };

        // Set up virtqueues
        for i in 0..self.get_virtqueue_count() {
            if !self.setup_queue(i, self.get_virtqueue_size(i)) {
                self.set_failed();
                return Err("Failed to set up virtqueue");
            }
        }

        // Mark driver OK
        self.driver_ok();
        self.debug_dump_mmio_state("init:after_driver_ok");
        Ok(negotiated_features)
    }

    fn is_modern_device(&self) -> bool {
        self.read32_register(Register::Version) == 2
    }

    fn supports_feature(&self, feature: u32) -> bool {
        let selector = feature / 32;
        let bit = feature % 32;
        self.write32_register(Register::DeviceFeaturesSel, selector);
        let device_features = self.read32_register(Register::DeviceFeatures);
        (device_features & (1u32 << bit)) != 0
    }

    /// Reset the device by writing 0 to the Status register
    fn reset(&mut self) -> Result<(), &'static str> {
        // self.debug_dump_mmio_state("reset:before");

        let _old = self.read32_register(Register::Status);
        self.write32_register(Register::Status, 0);
        // Ensure the write is visible to the device before we continue.
        io_mb();

        // let rb = self.read32_register(Register::Status);
        // self.debug_log_status_transition("reset", old, 0, rb);

        // Spec: wait until the device reports status==0.
        // Use a bounded loop so we never hang permanently.
        if cfg!(debug_assertions) {
            early_println!("[virtio][reset] waiting for reset completion...");
        }
        self.wait_for_status_zero("reset", 100_000)?;

        // self.debug_dump_mmio_state("reset:after");
        Ok(())
    }

    /// Set ACKNOWLEDGE status bit
    fn acknowledge(&mut self) {
        let old = self.read32_register(Register::Status);
        let mut status = old;
        DeviceStatus::Acknowledge.set(&mut status);
        self.write32_register(Register::Status, status);

        let rb = self.read32_register(Register::Status);
        self.debug_log_status_transition("ack", old, status, rb);
    }

    /// Set DRIVER status bit
    fn driver(&mut self) {
        let old = self.read32_register(Register::Status);
        let mut status = old;
        DeviceStatus::Driver.set(&mut status);
        self.write32_register(Register::Status, status);

        let rb = self.read32_register(Register::Status);
        self.debug_log_status_transition("driver", old, status, rb);
    }

    /// Set DRIVER_OK status bit
    fn driver_ok(&mut self) {
        let old = self.read32_register(Register::Status);
        let mut status = old;
        DeviceStatus::DriverOK.set(&mut status);
        self.write32_register(Register::Status, status);

        let rb = self.read32_register(Register::Status);
        self.debug_log_status_transition("driver_ok", old, status, rb);
    }

    /// Set FAILED status bit
    fn set_failed(&mut self) {
        let old = self.read32_register(Register::Status);
        let mut status = old;
        DeviceStatus::Failed.set(&mut status);
        self.write32_register(Register::Status, status);

        let rb = self.read32_register(Register::Status);
        self.debug_log_status_transition("failed", old, status, rb);
    }

    /// Negotiate device features
    ///
    /// This method reads device features, selects supported features,
    /// sets driver features, and verifies features OK status.
    ///
    /// # Returns
    ///
    /// Returns Ok(negotiated_features) if feature negotiation was successful,
    /// Err message otherwise
    fn negotiate_features(&mut self) -> Result<u32, &'static str> {
        // Read device features
        let device_features = self.read32_register(Register::DeviceFeatures);
        // Select supported features
        let driver_features = self.get_supported_features(device_features);
        crate::early_println!(
            "[virtio][feat] device_features=0x{:08x} driver_features=0x{:08x}",
            device_features,
            driver_features
        );

        #[cfg(test)]
        {
            use crate::early_println;
            early_println!(
                "[virtio] Negotiating features: device=0x{:x}, driver=0x{:x}",
                device_features,
                driver_features
            );
        }

        // Write driver features
        self.write32_register(Register::DriverFeatures, driver_features);

        // Set FEATURES_OK status bit
        let mut status = self.read32_register(Register::Status);
        DeviceStatus::FeaturesOK.set(&mut status);
        self.write32_register(Register::Status, status);

        // Verify FEATURES_OK status bit
        let final_status = self.read32_register(Register::Status);
        let success = DeviceStatus::FeaturesOK.is_set(final_status);

        #[cfg(test)]
        {
            use crate::early_println;
            early_println!(
                "[virtio] Feature negotiation result: success={}, status=0x{:x}",
                success,
                final_status
            );
        }

        if success {
            Ok(driver_features)
        } else {
            Err("Feature negotiation failed")
        }
    }

    /// Get device features supported by this driver
    ///
    /// This method can be overridden by specific device implementations
    /// to select which features to support.
    ///
    /// # Arguments
    ///
    /// * `device_features` - The features offered by the device
    ///
    /// # Returns
    ///
    /// The features supported by the driver
    fn get_supported_features(&self, device_features: u32) -> u32 {
        // By default, accept all device features
        // Device-specific implementations should override this
        device_features
    }

    fn allow_ring_features(&self) -> bool {
        self.is_modern_device()
            && self.supports_feature(crate::drivers::virtio::features::VIRTIO_F_VERSION_1)
    }

    /// Set up a virtqueue
    ///
    /// This method configures a virtqueue by setting the queue selection,
    /// size, alignment, and ready status.
    ///
    /// # Arguments
    ///
    /// * `queue_idx` - The index of the queue to set up
    ///
    /// # Returns
    ///
    /// Returns true if queue setup was successful, false otherwise
    fn setup_queue(&mut self, queue_idx: usize, queue_size: usize) -> bool {
        if queue_idx >= self.get_virtqueue_count() {
            return false;
        }

        // Select the queue
        self.write32_register(Register::QueueSel, queue_idx as u32);
        // Check if the queue is ready
        let ready = self.read32_register(Register::QueueReady);
        if ready != 0 {
            return false; // Queue already set up
        }

        // Get maximum queue size
        let queue_size_max = self.read32_register(Register::QueueNumMax);
        if queue_size > queue_size_max as usize {
            return false; // Requested size exceeds maximum
        }

        // Set queue size
        self.write32_register(Register::QueueNum, queue_size as u32);

        // Get queue addresses directly - safer than closures
        let desc_addr = self.get_queue_desc_addr(queue_idx);
        let driver_addr = self.get_queue_driver_addr(queue_idx);
        let device_addr = self.get_queue_device_addr(queue_idx);

        if desc_addr.is_none() || driver_addr.is_none() || device_addr.is_none() {
            return false;
        }

        let desc_addr = desc_addr.unwrap();
        let driver_addr = driver_addr.unwrap();
        let device_addr = device_addr.unwrap();

        // Set the queue descriptor address
        let desc_addr_low = (desc_addr & 0xffffffff) as u32;
        let desc_addr_high = (desc_addr >> 32) as u32;
        self.write32_register(Register::QueueDescLow, desc_addr_low);
        self.write32_register(Register::QueueDescHigh, desc_addr_high);

        // Set the driver area (available ring) address
        let driver_addr_low = (driver_addr & 0xffffffff) as u32;
        let driver_addr_high = (driver_addr >> 32) as u32;
        self.write32_register(Register::DriverDescLow, driver_addr_low);
        self.write32_register(Register::DriverDescHigh, driver_addr_high);

        // Set the device area (used ring) address
        let device_addr_low = (device_addr & 0xffffffff) as u32;
        let device_addr_high = (device_addr >> 32) as u32;
        self.write32_register(Register::DeviceDescLow, device_addr_low);
        self.write32_register(Register::DeviceDescHigh, device_addr_high);

        // Check the status of the queue
        let status = self.read32_register(Register::Status);
        if DeviceStatus::Failed.is_set(status) {
            return false; // Queue setup failed
        }

        // Mark queue as ready
        self.write32_register(Register::QueueReady, 1);

        // Check the status of the queue
        let status = self.read32_register(Register::Status);
        if DeviceStatus::Failed.is_set(status) {
            return false; // Queue setup failed
        }

        true
    }

    /// Read device-specific configuration
    ///
    /// This method reads configuration data from the device-specific configuration space.
    ///
    /// # Arguments
    ///
    /// * `offset` - The offset within the configuration space
    ///
    /// # Returns
    ///
    /// The configuration value of type T
    fn read_config<T: Sized>(&self, offset: usize) -> T {
        let addr = self.get_base_addr() + Register::DeviceConfig.offset() + offset;
        // Prefer single-instruction sized accesses for MMIO on AArch64/HVF.
        // Fall back to byte-wise access for unusual sizes.
        unsafe {
            match core::mem::size_of::<T>() {
                1 => {
                    let v = crate::arch::mmio::read8(addr);
                    core::mem::transmute_copy::<u8, T>(&v)
                }
                2 => {
                    let v = crate::arch::mmio::read16(addr);
                    core::mem::transmute_copy::<u16, T>(&v)
                }
                4 => {
                    let v = crate::arch::mmio::read32(addr);
                    core::mem::transmute_copy::<u32, T>(&v)
                }
                8 => {
                    let v = crate::arch::mmio::read64(addr);
                    core::mem::transmute_copy::<u64, T>(&v)
                }
                _ => {
                    let mut out = core::mem::MaybeUninit::<T>::uninit();
                    let dst = out.as_mut_ptr() as *mut u8;
                    for i in 0..core::mem::size_of::<T>() {
                        let b = crate::arch::mmio::read8(addr + i);
                        core::ptr::write(dst.add(i), b);
                    }
                    out.assume_init()
                }
            }
        }
    }

    /// Write device-specific configuration
    ///
    /// This method writes configuration data to the device-specific configuration space.
    ///
    /// # Arguments
    ///
    /// * `offset` - The offset within the configuration space
    /// * `value` - The value to write
    fn write_config<T: Sized>(&self, offset: usize, value: T) {
        let addr = self.get_base_addr() + Register::DeviceConfig.offset() + offset;
        // Prefer single-instruction sized accesses for MMIO on AArch64/HVF.
        // Fall back to byte-wise access for unusual sizes.
        unsafe {
            match core::mem::size_of::<T>() {
                1 => {
                    let v = core::mem::transmute_copy::<T, u8>(&value);
                    crate::arch::mmio::write8(addr, v);
                }
                2 => {
                    let v = core::mem::transmute_copy::<T, u16>(&value);
                    crate::arch::mmio::write16(addr, v);
                }
                4 => {
                    let v = core::mem::transmute_copy::<T, u32>(&value);
                    crate::arch::mmio::write32(addr, v);
                }
                8 => {
                    let v = core::mem::transmute_copy::<T, u64>(&value);
                    crate::arch::mmio::write64(addr, v);
                }
                _ => {
                    let src = &value as *const T as *const u8;
                    for i in 0..core::mem::size_of::<T>() {
                        let b = core::ptr::read(src.add(i));
                        crate::arch::mmio::write8(addr + i, b);
                    }
                }
            }
        }
    }

    /// Get device and vendor IDs
    ///
    /// # Returns
    ///
    /// A tuple containing (device_id, vendor_id)
    fn get_device_info(&self) -> (u32, u32) {
        let device_id = self.read32_register(Register::DeviceId);
        let vendor_id = self.read32_register(Register::VendorId);
        (device_id, vendor_id)
    }

    /// Get interrupt status
    ///
    /// # Returns
    ///
    /// The interrupt status register value
    fn get_interrupt_status(&self) -> u32 {
        self.read32_register(Register::InterruptStatus)
    }

    /// Process interrupts (polling method)
    ///
    /// This method checks for interrupts and acknowledges them.
    ///
    /// # Returns
    ///
    /// The interrupt status before acknowledgment
    fn process_interrupts(&mut self) -> u32 {
        let status = self.get_interrupt_status();
        if status != 0 {
            self.write32_register(Register::InterruptAck, status & 0x03);
        }
        status
    }

    /// Memory barrier for ensuring memory operations ordering
    fn memory_barrier(&self) {
        // Virtio requires ordering of normal memory (descriptor writes) vs MMIO doorbells.
        // On RISC-V, a plain atomic fence may not order memory vs device I/O; use an I/O fence.
        crate::arch::io_mb();
    }

    /// Notify the device about new buffers in a specified virtqueue
    ///
    /// This method notifies the device that new buffers are available in the specified virtqueue.
    /// It selects the queue using the QueueSel register and then writes to the QueueNotify register.
    ///
    /// # Arguments
    ///
    /// * `virtqueue_idx` - The index of the virtqueue to notify
    ///
    /// # Panics
    ///
    /// Panics if the virtqueue index is invalid
    fn notify(&self, virtqueue_idx: usize) {
        if virtqueue_idx >= self.get_virtqueue_count() {
            panic!("Invalid virtqueue index");
        }
        // Insert memory barrier before notification
        io_mb();
        self.write32_register(Register::QueueNotify, virtqueue_idx as u32);
        io_mb();
    }

    /// Read a 32-bit value from a device register
    ///
    /// # Arguments
    ///
    /// * `register` - The register to read from
    ///
    /// # Returns
    ///
    /// The 32-bit value read from the register
    fn read32_register(&self, register: Register) -> u32 {
        let addr = self.get_base_addr() + register.offset();
        io_mb();
        let val = unsafe { crate::arch::mmio::read32(addr) };
        io_mb();
        val
    }

    /// Write a 32-bit value to a device register
    ///
    /// # Arguments
    ///
    /// * `register` - The register to write to
    /// * `value` - The 32-bit value to write
    fn write32_register(&self, register: Register, value: u32) {
        let addr = self.get_base_addr() + register.offset();
        // NOTE: Release builds on some environments have shown sensitivity to MMIO
        // sequencing/posted writes. Use a non-inlined slowpath with a readback flush
        // to reduce optimization/timing variance.
        #[cfg(not(debug_assertions))]
        {
            self.write32_register_slowpath(addr, value);
        }

        #[cfg(debug_assertions)]
        {
            io_mb();
            unsafe { crate::arch::mmio::write32(addr, value) };
            io_mb();
        }

        if register == Register::Status && (value & !0xff) != 0 {
            crate::early_println!(
                "[virtio][WARN] writing non-8bit value to Status: 0x{:08x} (base={:#x})",
                value,
                self.get_base_addr()
            );
        }
    }

    /// Read a 64-bit value from a device register
    ///
    /// # Arguments
    ///
    /// * `register` - The register to read from
    ///
    /// # Returns
    ///
    /// The 64-bit value read from the register
    fn read64_register(&self, register: Register) -> u64 {
        let addr = self.get_base_addr() + register.offset();
        io_mb();
        let val = unsafe { crate::arch::mmio::read64(addr) };
        io_mb();
        val
    }

    /// Write a 64-bit value to a device register
    ///
    /// # Arguments
    ///
    /// * `register` - The register to write to
    /// * `value` - The 64-bit value to write
    fn write64_register(&self, register: Register, value: u64) {
        let addr = self.get_base_addr() + register.offset();
        io_mb();
        unsafe { crate::arch::mmio::write64(addr, value) };
        io_mb();
    }

    // Required methods to be implemented by specific device types

    fn get_base_addr(&self) -> usize;
    fn get_virtqueue_count(&self) -> usize;
    fn get_virtqueue_size(&self, queue_idx: usize) -> usize;

    /// Get the descriptor address for a virtqueue
    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64>;

    /// Get the driver area address for a virtqueue
    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64>;

    /// Get the device area address for a virtqueue
    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64>;
}

/// Device type enum for Virtio devices
///
/// This enum represents the different types of Virtio devices.
/// Each variant corresponds to a specific device type.
/// The types are defined in the Virtio specification.
pub enum VirtioDeviceType {
    Invalid = 0,
    Net = 1,
    Block = 2,
    Console = 3,
    Rng = 4,
    GPU = 16,
    Input = 18,
}

impl VirtioDeviceType {
    /// Convert from u32 to VirtioDeviceType
    ///
    /// This method converts a u32 value to the corresponding VirtioDeviceType variant.
    ///
    /// # Arguments
    ///
    /// * `device_type` - The u32 value to convert.
    ///
    /// # Returns
    ///
    /// Returns the corresponding VirtioDeviceType variant.
    pub fn from_u32(device_type: u32) -> Self {
        match device_type {
            0 => VirtioDeviceType::Invalid,
            1 => VirtioDeviceType::Net,
            2 => VirtioDeviceType::Block,
            3 => VirtioDeviceType::Console,
            4 => VirtioDeviceType::Rng,
            16 => VirtioDeviceType::GPU,
            18 => VirtioDeviceType::Input,
            _ => panic!("Not supported device type"),
        }
    }
}

/// Virtio Common Device
///
/// Only use this struct for checking the device info.
/// It should not be used for actual device operations.
///
struct VirtioDeviceCommon {
    base_addr: usize,
}

impl VirtioDeviceCommon {
    /// Create a new Virtio device
    ///
    /// # Arguments
    ///
    /// * `base_addr` - The base address of the device
    ///
    /// # Returns
    ///
    /// A new instance of `VirtioDeviceCommon`
    pub fn new(base_addr: usize) -> Self {
        Self { base_addr }
    }
}

impl VirtioDevice for VirtioDeviceCommon {
    fn init(&mut self) -> Result<u32, &'static str> {
        // Initialization is not required for the common device
        Ok(0)
    }

    fn get_base_addr(&self) -> usize {
        self.base_addr
    }

    fn get_virtqueue_count(&self) -> usize {
        // This should be overridden by specific device implementations
        0
    }

    fn get_virtqueue_size(&self, _queue_idx: usize) -> usize {
        // This should be overridden by specific device implementations
        0
    }

    fn get_queue_desc_addr(&self, _queue_idx: usize) -> Option<u64> {
        // This should be overridden by specific device implementations
        None
    }

    fn get_queue_driver_addr(&self, _queue_idx: usize) -> Option<u64> {
        // This should be overridden by specific device implementations
        None
    }

    fn get_queue_device_addr(&self, _queue_idx: usize) -> Option<u64> {
        // This should be overridden by specific device implementations
        None
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let res = device.get_resources();
    if res.is_empty() {
        return Err("No resources found");
    }

    // Get memory region resource (res_type == PlatformDeviceResourceType::MEM)
    let mem_res = res
        .iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("Memory resource not found")?;

    let base_addr = mem_res.start as usize;

    // Create a new Virtio device
    let virtio_device = VirtioDeviceCommon::new(base_addr);
    // Check device type
    let device_type = VirtioDeviceType::from_u32(virtio_device.get_device_info().0);

    match device_type {
        VirtioDeviceType::Block => {
            let id = BLOCK_COUNTER.fetch_add(1, Ordering::SeqCst);
            let name = format!("vblk{}", id);
            crate::early_println!(
                "[Virtio] Detected Virtio Block Device at {:#x}, registering as {}",
                base_addr,
                name
            );
            let dev: Arc<dyn Device> = Arc::new(VirtioBlockDevice::new(base_addr));
            DeviceManager::get_manager().register_device_with_name(name, dev);
        }
        VirtioDeviceType::Net => {
            let id = NET_COUNTER.fetch_add(1, Ordering::SeqCst);
            let name = format!("veth{}", id);
            crate::early_println!(
                "[Virtio] Detected Virtio Network Device at {:#x}, registering as {}",
                base_addr,
                name
            );
            let dev = Arc::new(VirtioNetDevice::new(base_addr));
            dev.register_interface(&name);

            // Register interrupt handler if IRQ resource is available
            if let Some(irq_resource) = device
                .get_resources()
                .iter()
                .find(|r| r.res_type == PlatformDeviceResourceType::IRQ)
            {
                let interrupt_id = irq_resource.start as u32;
                crate::early_println!("[Virtio] Net device interrupt ID: {}", interrupt_id);

                if let Err(e) = dev.enable_interrupts(interrupt_id) {
                    crate::early_println!("[Virtio] Failed to enable net interrupts: {}", e);
                } else if let Err(e) = crate::interrupt::InterruptManager::with_manager(|mgr| {
                    mgr.register_interrupt_device(interrupt_id, dev.clone())
                }) {
                    crate::early_println!(
                        "[Virtio] Failed to register net interrupt device: {}",
                        e
                    );
                } else {
                    crate::early_println!("[Virtio] Net interrupt device registered");
                }
            } else {
                crate::early_println!("[Virtio] No interrupt resource found for net device");
            }

            DeviceManager::get_manager().register_device_with_name(name, dev);
        }
        VirtioDeviceType::GPU => {
            let id = GPU_COUNTER.fetch_add(1, Ordering::SeqCst);
            let name = format!("vfb{}", id);
            crate::early_println!(
                "[Virtio] Detected Virtio GPU Device at {:#x}, registering as {}",
                base_addr,
                name
            );
            let dev: Arc<dyn Device> = Arc::new(VirtioGpuDevice::new(base_addr));
            DeviceManager::get_manager().register_device_with_name(name, dev);
        }
        VirtioDeviceType::Input => {
            crate::early_println!("[Virtio] Detected Virtio Input Device at {:#x}", base_addr);
            // Create VirtIO Input device
            let dev = Arc::new(VirtioInputDevice::new(base_addr));

            // Register interrupt handler if IRQ resource is available
            if let Some(irq_resource) = device
                .get_resources()
                .iter()
                .find(|r| r.res_type == PlatformDeviceResourceType::IRQ)
            {
                let interrupt_id = irq_resource.start as u32;
                crate::early_println!("[Virtio] Input device interrupt ID: {}", interrupt_id);

                // Enable interrupts
                if let Err(e) = dev.enable_interrupts(interrupt_id) {
                    crate::early_println!("[Virtio] Failed to enable input interrupts: {}", e);
                } else {
                    crate::early_println!(
                        "[Virtio] Input interrupts enabled (ID: {})",
                        interrupt_id
                    );

                    // Register interrupt handler
                    if let Err(e) = crate::interrupt::InterruptManager::with_manager(|mgr| {
                        mgr.register_interrupt_device(interrupt_id, dev.clone())
                    }) {
                        crate::early_println!(
                            "[Virtio] Failed to register input interrupt device: {}",
                            e
                        );
                    } else {
                        crate::early_println!("[Virtio] Input interrupt device registered");
                    }
                }
            } else {
                crate::early_println!("[Virtio] No interrupt resource found for input device");
            }

            // Keep device alive by registering with DeviceManager
            DeviceManager::get_manager().register_device(dev);
        }
        VirtioDeviceType::Rng => {
            let id = RNG_COUNTER.fetch_add(1, Ordering::SeqCst);
            crate::early_println!("[Virtio] Detected Virtio RNG Device at {:#x}", base_addr);

            // Create and register the VirtIO RNG device as an entropy source
            let rng_device = Arc::new(VirtioRngDevice::new(base_addr));
            crate::random::RandomManager::register_entropy_source(rng_device);

            // Register the RandomCharDevice as /dev/random (only for the first RNG device)
            if id == 0 {
                let random_char_dev: Arc<dyn Device> =
                    Arc::new(crate::random::RandomCharDevice::new());
                DeviceManager::get_manager()
                    .register_device_with_name("random".to_string(), random_char_dev);
                crate::early_println!("[Virtio] Registered /dev/random character device");
            }
        }
        _ => {
            // Unsupported device type
            return Err("Unsupported device type");
        }
    }

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new("virtio-mmio", probe_fn, remove_fn, vec!["virtio,mmio"]);
    // Register the driver with the kernel
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Standard)
}

driver_initcall!(register_driver);

#[cfg(test)]
mod tests;
