//! Platform device resource management module.
//!
//! This module defines the `PlatformDeviceResource` struct and the `PlatformDeviceResourceType` enum,
//! which represent the resources associated with platform devices.
//!

/// PlatformDeviceResource struct
///
/// This struct represents a resource associated with a platform device.
/// It contains the resource type (memory, I/O, IRQ, or DMA),
/// the starting address, and the ending address of the resource.
#[derive(Debug)]
pub struct PlatformDeviceResource {
    pub res_type: PlatformDeviceResourceType,
    pub start: usize,
    pub end: usize,
    /// Optional metadata for IRQ resources (e.g., type, flags from Device Tree)
    pub irq_metadata: Option<IrqMetadata>,
}

/// IRQ metadata from Device Tree interrupt specifiers
#[derive(Debug, Clone, Copy)]
pub struct IrqMetadata {
    /// Interrupt type (e.g., 0=SPI, 1=PPI for ARM GIC)
    pub irq_type: u32,
    /// Interrupt number (before controller-specific translation)
    pub irq_number: u32,
    /// Interrupt flags (e.g., trigger type, polarity)
    pub irq_flags: u32,
}

/// PlatformDeviceResourceType enum
///
/// This enum defines the types of resources that can be associated with a platform device.
/// The types include memory (MEM), I/O (IO), interrupt request (IRQ), and direct memory access (DMA).
/// Each type is represented as a variant of the enum.
#[derive(PartialEq, Eq, Debug)]
pub enum PlatformDeviceResourceType {
    MEM,
    IO,
    IRQ,
    DMA,
}
